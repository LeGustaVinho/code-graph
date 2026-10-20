//! Rust module resolution inputs: where a file sits in its crate's module tree (from the
//! Cargo layout) and the crate names declared by `Cargo.toml` manifests.

use std::collections::HashMap;
use std::path::Path;

/// Module of a Rust file: (crate key, module path inside the crate).
///
/// The crate key is the package directory (`crates/core`, `""` at the root); binaries,
/// integration tests, examples and benches are crates of their own (`crates/core/bin/tool`,
/// `crates/core/tests`). `src/lib.rs` / `src/main.rs` are the root module (`[]`),
/// `src/a/b.rs` and `src/a/b/mod.rs` are `a::b`.
pub fn module_path(file: &str) -> (String, Vec<String>) {
    let stem = file.strip_suffix(".rs").unwrap_or(file);
    let parts: Vec<&str> = stem.split('/').collect();
    let dir_of = |i: usize| parts[..i].join("/");
    let tail = |from: usize| -> Vec<String> {
        let mut v: Vec<String> = parts[from..].iter().map(|s| s.to_string()).collect();
        if v.last().map(|l| l == "mod").unwrap_or(false) {
            v.pop();
        }
        v
    };
    if let Some(i) = parts.iter().rposition(|p| *p == "src") {
        let crate_dir = dir_of(i);
        let rel = &parts[i + 1..];
        if rel.first() == Some(&"bin") && rel.len() >= 2 {
            let key = join_key(&crate_dir, &format!("bin/{}", rel[1]));
            let mut path = tail(i + 3);
            if path == ["main"] {
                path.clear();
            }
            return (key, path);
        }
        if rel == ["lib"] || rel == ["main"] {
            return (crate_dir, Vec::new());
        }
        return (crate_dir, tail(i + 1));
    }
    if let Some(i) = parts.iter().rposition(|p| matches!(*p, "tests" | "examples" | "benches")) {
        let key = join_key(&dir_of(i), parts[i]);
        // Each top-level file is a crate root; sub-directories are its modules.
        let path = if parts.len() == i + 2 { Vec::new() } else { tail(i + 1) };
        return (key, path);
    }
    // Loose file (`build.rs`, scripts): a crate of its own.
    (stem.to_string(), Vec::new())
}

/// A crate root that shares its crate key with sibling roots: each top-level file of
/// `tests/`, `examples/` and `benches/` is a crate of its own, so none of them is the module
/// that `crate::` names for the others.
pub fn is_shared_root(file: &str) -> bool {
    let parts: Vec<&str> = file.split('/').collect();
    !parts.contains(&"src") && parts.len() >= 2 && matches!(parts[parts.len() - 2], "tests" | "examples" | "benches")
}

fn join_key(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// Package directories (where `Cargo.toml` lives) of the crates holding these Rust files.
pub fn crate_dirs<'a>(files: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut dirs: Vec<String> = files
        .into_iter()
        .filter_map(|f| {
            let parts: Vec<&str> = f.split('/').collect();
            let i = parts.iter().rposition(|p| *p == "src")?;
            Some(parts[..i].join("/"))
        })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs
}

/// Crate name → crate key, from the `Cargo.toml` of each package directory (`[lib] name` or
/// `[package] name`, `-` read as `_`); the directory name stands in when there is none.
pub fn load_crates(root: &Path, crate_dirs: &[String]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut dirs = crate_dirs.to_vec();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        let manifest = if dir.is_empty() { root.join("Cargo.toml") } else { root.join(&dir).join("Cargo.toml") };
        let names = std::fs::read_to_string(&manifest).ok().and_then(|t| t.parse::<toml::Table>().ok()).map(|t| {
            let get = |sect: &str| t.get(sect).and_then(|s| s.get("name")).and_then(|n| n.as_str()).map(|s| s.replace('-', "_"));
            [get("lib"), get("package")].into_iter().flatten().collect::<Vec<_>>()
        });
        let names = match names {
            Some(n) if !n.is_empty() => n,
            _ => {
                let fallback = if dir.is_empty() {
                    root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                } else {
                    dir.rsplit('/').next().unwrap_or(&dir).to_string()
                };
                vec![fallback.replace('-', "_")]
            }
        };
        for n in names {
            out.entry(n).or_insert_with(|| dir.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        assert_eq!(module_path("src/lib.rs"), ("".into(), vec![]));
        assert_eq!(module_path("crates/core/src/extract/mod.rs"), ("crates/core".into(), vec!["extract".to_string()]));
        assert_eq!(module_path("crates/core/src/extract/rust.rs"), ("crates/core".into(), vec!["extract".to_string(), "rust".to_string()]));
        assert_eq!(module_path("src/bin/tool.rs"), ("bin/tool".into(), vec![]));
        assert_eq!(module_path("src/bin/tool/args.rs"), ("bin/tool".into(), vec!["args".to_string()]));
        assert_eq!(module_path("tests/api.rs"), ("tests".into(), vec![]));
        assert_eq!(module_path("crates/x/tests/common/mod.rs"), ("crates/x/tests".into(), vec!["common".to_string()]));
    }
}
