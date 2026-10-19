//! Source discovery honoring `.gitignore` plus well-known build/vendor folders.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::lang::Language;

pub const SKIP_DIRS: &[&str] = &[
    ".git",
    ".codegraph",
    "node_modules",
    "bin",
    "obj",
    "dist",
    "build",
    "out",
    "target",
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

pub fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Returns true when `rel_path` lies inside a directory that discovery skips.
pub fn is_ignored_rel_path(rel_path: &str) -> bool {
    let mut parts: Vec<&str> = rel_path.split('/').collect();
    parts.pop();
    parts.iter().any(|p| is_skipped_dir(p))
}

/// Walks `root` and returns every indexable source file.
pub fn discover(root: &Path) -> Vec<SourceFile> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|e| {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && e.depth() > 0 {
                let name = e.file_name().to_string_lossy();
                return !is_skipped_dir(&name);
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
