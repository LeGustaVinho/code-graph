//! Source discovery honoring `.gitignore` / `.ignore` / `.codegraphignore` plus well-known
//! build, tooling and engine folders. Discovery, the file watcher and incremental
//! re-indexing share one rule set ([`IgnoreRules`]) so they always agree on what is indexed.
//! [`VendorRules`] tells third-party code (indexed, but not the project's own) apart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::WalkBuilder;

use crate::lang::Language;

/// Directories skipped wherever they appear.
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    ".codegraph",
    "node_modules",
    "obj",
    ".next",
    ".nuxt",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".idea",
    ".vs",
    ".vscode",
    "wwwroot",
];

/// Build-output folder names: skipped at the project root or next to a build manifest
/// (`bin/` beside a `.csproj`, `dist/` beside a `package.json`), but not deeper in a tree
/// where they are ordinary source folders (`Assets/Editor/Build/`).
pub const BUILD_DIRS: &[&str] = &["bin", "build", "out", "dist", "target"];

const MANIFEST_NAMES: &[&str] =
    &["package.json", "Cargo.toml", "pyproject.toml", "setup.py", "build.gradle", "build.gradle.kts", "pom.xml", "go.mod"];
const MANIFEST_EXTS: &[&str] = &[".csproj", ".fsproj", ".vbproj", ".sln"];

/// Folders the Unity editor owns at the root of a Unity project.
const UNITY_ROOT_DIRS: &[&str] = &["Library", "Temp", "Logs", "UserSettings", "MemoryCaptures", "Build", "Builds", "Obj"];

/// Files larger than this are skipped (generated bundles, dumps).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub abs_path: PathBuf,
    pub rel_path: String,
    pub language: Language,
    pub mtime: i64,
    pub size: u64,
}

pub fn normalize_rel(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let s = rel.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("./").to_string()
}

pub fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Directory names skipped anywhere (does not include root-only build folders).
pub fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Path-only check (no ignore files): inside a directory skipped anywhere.
pub fn is_ignored_rel_path(rel_path: &str) -> bool {
    let mut parts: Vec<&str> = rel_path.split('/').collect();
    parts.pop();
    parts.iter().any(|p| is_skipped_dir(p))
}

/// Is `root` a Unity project?
pub fn is_unity_project(root: &Path) -> bool {
    root.join("ProjectSettings").join("ProjectVersion.txt").is_file()
}

/// The complete ignore rule set of a project.
pub struct IgnoreRules {
    root: PathBuf,
    ignore_files: Option<Gitignore>,
    unity: bool,
    /// Directory (relative) → does it hold a build manifest?
    manifests: Mutex<HashMap<String, bool>>,
}

impl IgnoreRules {
    pub fn load(root: &Path) -> IgnoreRules {
        let mut b = GitignoreBuilder::new(root);
        let mut any = false;
        for f in [".gitignore", ".ignore", ".codegraphignore"] {
            let p = root.join(f);
            if p.is_file() && b.add(&p).is_none() {
                any = true;
            }
        }
        let ignore_files = if any { b.build().ok() } else { None };
        IgnoreRules { root: root.to_path_buf(), ignore_files, unity: is_unity_project(root), manifests: Mutex::new(HashMap::new()) }
    }

    fn has_manifest(&self, rel_dir: &str) -> bool {
        if let Some(v) = self.manifests.lock().unwrap().get(rel_dir) {
            return *v;
        }
        let abs = if rel_dir.is_empty() { self.root.clone() } else { self.root.join(rel_dir) };
        let found = std::fs::read_dir(&abs)
            .map(|rd| {
                rd.flatten().any(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    MANIFEST_NAMES.contains(&n.as_str()) || MANIFEST_EXTS.iter().any(|x| n.ends_with(x))
                })
            })
            .unwrap_or(false);
        self.manifests.lock().unwrap().insert(rel_dir.to_string(), found);
        found
    }

    /// Is the directory `rel_dir` (project-relative, `/`-separated) skipped?
    pub fn dir_ignored(&self, rel_dir: &str) -> bool {
        let segs: Vec<&str> = rel_dir.split('/').filter(|s| !s.is_empty()).collect();
        let Some(name) = segs.last() else { return false };
        if is_skipped_dir(name) {
            return true;
        }
        let parent = segs[..segs.len() - 1].join("/");
        if BUILD_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name)) && (segs.len() == 1 || self.has_manifest(&parent)) {
            return true;
        }
        if self.unity && segs.len() == 1 && UNITY_ROOT_DIRS.contains(name) {
            return true;
        }
        false
    }

    /// Is the file `rel_path` excluded (any ignored ancestor directory, or ignore files)?
    pub fn is_ignored(&self, rel_path: &str) -> bool {
        let segs: Vec<&str> = rel_path.split('/').collect();
        for i in 1..segs.len() {
            if self.dir_ignored(&segs[..i].join("/")) {
                return true;
            }
        }
        match &self.ignore_files {
            Some(g) => g.matched_path_or_any_parents(self.root.join(rel_path), false).is_ignore(),
            None => false,
        }
    }
}

/// Third-party code: indexed and resolvable, but not the project's own (dead-code, hotspot
/// and overview reports leave it out). Defaults plus `.codegraph/config.toml`:
///
/// ```toml
/// vendor = ["Assets/ThirdPartyStuff/**"]
/// not_vendor = ["Packages/com.mycompany.*/**"]
/// ```
pub struct VendorRules {
    vendor: GlobSet,
    not_vendor: GlobSet,
}

#[derive(serde::Deserialize, Default)]
struct ProjectConfig {
    #[serde(default)]
    vendor: Vec<String>,
    #[serde(default)]
    not_vendor: Vec<String>,
}

impl VendorRules {
    pub fn load(root: &Path) -> VendorRules {
        let mut patterns: Vec<String> = ["**/ThirdParty/**", "**/third_party/**", "**/third-party/**", "**/vendor/**", "**/External/**"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if is_unity_project(root) {
            patterns.push("Assets/Plugins/**".into());
            patterns.push("Packages/**".into());
        }
        let cfg: ProjectConfig = std::fs::read_to_string(root.join(".codegraph").join("config.toml"))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        patterns.extend(cfg.vendor);
        VendorRules { vendor: globs(&patterns), not_vendor: globs(&cfg.not_vendor) }
    }

    pub fn is_vendor(&self, rel_path: &str) -> bool {
        self.vendor.is_match(rel_path) && !self.not_vendor.is_match(rel_path)
    }
}

fn globs(patterns: &[String]) -> GlobSet {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = GlobBuilder::new(p).case_insensitive(true).literal_separator(true).build() {
            b.add(g);
        }
    }
    b.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Walks `root` and returns every indexable source file.
pub fn discover(root: &Path) -> Vec<SourceFile> {
    discover_counting(root, &std::sync::atomic::AtomicUsize::new(0))
}

/// [`discover`], counting files found into `found` as the walk goes (progress display).
pub fn discover_counting(root: &Path, found: &std::sync::atomic::AtomicUsize) -> Vec<SourceFile> {
    let rules = std::sync::Arc::new(IgnoreRules::load(root));
    let filter_rules = rules.clone();
    let filter_root = root.to_path_buf();
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .add_custom_ignore_filename(".codegraphignore")
        .filter_entry(move |e| {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.depth() > 0 {
                return !filter_rules.dir_ignored(&normalize_rel(&filter_root, e.path()));
            }
            true
        })
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let Some(language) = Language::from_path(path) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        found.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        out.push(SourceFile {
            abs_path: path.to_path_buf(),
            rel_path: normalize_rel(root, path),
            language,
            mtime: mtime_of(&meta),
            size: meta.len(),
        });
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_folders_only_at_root_or_beside_a_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("Assets/Editor/Build")).unwrap();
        std::fs::create_dir_all(root.join("api/bin")).unwrap();
        std::fs::write(root.join("api/api.csproj"), "<Project/>").unwrap();
        let rules = IgnoreRules::load(root);
        assert!(rules.dir_ignored("build"));
        assert!(rules.dir_ignored("api/bin"));
        assert!(!rules.dir_ignored("Assets/Editor/Build"));
        assert!(rules.dir_ignored("x/node_modules"));
    }

    #[test]
    fn vendor_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("ProjectSettings")).unwrap();
        std::fs::write(tmp.path().join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3").unwrap();
        let v = VendorRules::load(tmp.path());
        assert!(v.is_vendor("Assets/Plugins/DOTween/Tween.cs"));
        assert!(v.is_vendor("Packages/com.x/Runtime/A.cs"));
        assert!(!v.is_vendor("Assets/Game/Player.cs"));
    }
}
