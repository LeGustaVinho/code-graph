//! JS/TS module resolution inputs read from the project: `tsconfig*.json` / `jsconfig.json`
//! (`baseUrl`, `paths`, following `extends`) and `package.json` packages (monorepo
//! workspaces importing each other by name).

use std::path::Path;

use serde_json::Value;

use crate::index::normalize_path;

/// `baseUrl` / `paths` of one config, every path relative to the project root.
#[derive(Clone, Debug, Default)]
pub struct TsPaths {
    /// Directory of the config file ("" for the project root).
    pub dir: String,
    pub base_url: Option<String>,
    /// Pattern (`@/*`) → targets (`src/*`), in declaration order.
    pub paths: Vec<(String, Vec<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct JsPackage {
    pub name: String,
    pub dir: String,
    /// `source` / `module` / `main` / `types` / `exports["."]` entry files.
    pub entries: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct JsProject {
    pub configs: Vec<TsPaths>,
    pub packages: Vec<JsPackage>,
}

impl JsProject {
    /// The config governing `file`: the one in the nearest enclosing directory.
    pub fn nearest(&self, file: &str) -> Option<&TsPaths> {
        self.configs.iter().filter(|c| c.dir.is_empty() || file.starts_with(&format!("{}/", c.dir))).max_by_key(|c| c.dir.len())
    }
}

/// Matches `spec` against a `paths` pattern; returns what `*` captured ("" for exact patterns).
pub fn match_pattern<'s>(pattern: &str, spec: &'s str) -> Option<&'s str> {
    match pattern.split_once('*') {
        None => (pattern == spec).then_some(""),
        Some((pre, post)) => {
            if spec.len() >= pre.len() + post.len() && spec.starts_with(pre) && spec.ends_with(post) {
                Some(&spec[pre.len()..spec.len() - post.len()])
            } else {
                None
            }
        }
    }
}

const MAX_DEPTH: usize = 8;

pub fn load(root: &Path) -> JsProject {
    let mut project = JsProject::default();
    let mut files = Vec::new();
    let rules = codegraph_core::walk::IgnoreRules::load(root);
    collect(root, root, 0, &rules, &mut files);
    files.sort();
    for rel in files {
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        if name == "package.json" {
            if let Some(p) = read_json(&root.join(&rel)).and_then(|v| package(&v, &dir)) {
                project.packages.push(p);
            }
        } else if let Some(cfg) = tsconfig(root, &rel, 0) {
            if cfg.base_url.is_some() || !cfg.paths.is_empty() {
                project.configs.push(TsPaths { dir, ..cfg });
            }
        }
    }
    project
}

fn collect(root: &Path, dir: &Path, depth: usize, rules: &codegraph_core::walk::IgnoreRules, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let child = e.path().strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            if !name.starts_with('.') && !rules.dir_ignored(&child) {
                collect(root, &e.path(), depth + 1, rules, out);
            }
        } else if name == "package.json" || name == "jsconfig.json" || (name.starts_with("tsconfig") && name.ends_with(".json")) {
            let rel = e.path().strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            out.push(rel);
        }
    }
}

/// Reads a tsconfig (JSON with comments / trailing commas), following `extends`.
fn tsconfig(root: &Path, rel: &str, depth: usize) -> Option<TsPaths> {
    if depth > 4 {
        return None;
    }
    let v = read_json(&root.join(rel))?;
    let dir = rel.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let join = |base: &str, p: &str| normalize_path(&if base.is_empty() { p.to_string() } else { format!("{base}/{p}") });
    // Inherited settings first; this file overrides them.
    let mut out = TsPaths::default();
    let parents: Vec<String> = match v.get("extends") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
        _ => Vec::new(),
    };
    for p in parents.iter().filter(|p| p.starts_with('.')) {
        let mut target = join(&dir, p);
        if !target.ends_with(".json") {
            target.push_str(".json");
        }
        if let Some(parent) = tsconfig(root, &target, depth + 1) {
            out.base_url = parent.base_url.or(out.base_url);
            if !parent.paths.is_empty() {
                out.paths = parent.paths;
            }
        }
    }
    let opts = v.get("compilerOptions");
    if let Some(b) = opts.and_then(|o| o.get("baseUrl")).and_then(|b| b.as_str()) {
        out.base_url = Some(join(&dir, b));
    }
    if let Some(paths) = opts.and_then(|o| o.get("paths")).and_then(|p| p.as_object()) {
        // Targets are relative to baseUrl when set, otherwise to this config's directory.
        let base = out.base_url.clone().unwrap_or_else(|| dir.clone());
        out.paths = paths
            .iter()
            .map(|(pat, targets)| {
                let ts =
                    targets.as_array().map(|a| a.iter().filter_map(|t| t.as_str()).map(|t| join(&base, t)).collect()).unwrap_or_default();
                (pat.clone(), ts)
            })
            .collect();
    }
    Some(out)
}

fn package(v: &Value, dir: &str) -> Option<JsPackage> {
    let name = v.get("name")?.as_str()?.to_string();
    let mut entries = Vec::new();
    for key in ["source", "module", "main", "types", "typings"] {
        if let Some(e) = v.get(key).and_then(|e| e.as_str()) {
            entries.push(e.trim_start_matches("./").to_string());
        }
    }
    if let Some(dot) = v.get("exports").and_then(|e| e.get(".").or(Some(e))) {
        let mut stack = vec![dot];
        while let Some(x) = stack.pop() {
            match x {
                Value::String(s) if s.starts_with("./") => entries.push(s.trim_start_matches("./").to_string()),
                Value::Object(o) => stack.extend(o.values()),
                _ => {}
            }
        }
    }
    Some(JsPackage { name, dir: dir.to_string(), entries })
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc(&text)).ok()
}

/// Removes `//` and `/* */` comments (outside strings) and trailing commas.
fn strip_jsonc(src: &str) -> String {
    String::from_utf8_lossy(&strip_bytes(src.as_bytes())).into_owned()
}

/// Byte-level JSONC cleanup (multi-byte UTF-8 sequences pass through untouched).
fn strip_bytes(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => {
                in_str = true;
                out.push(c);
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            b',' => {
                let next = b[i + 1..].iter().find(|x| !x.is_ascii_whitespace());
                if !matches!(next, Some(b'}') | Some(b']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let v: Value = serde_json::from_str(&strip_jsonc("{ // c\n \"a\": \"x//y\", /* b */ \"b\": [1, 2,], }")).unwrap();
        assert_eq!(v["a"], "x//y");
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn patterns() {
        assert_eq!(match_pattern("@/*", "@/components/Btn"), Some("components/Btn"));
        assert_eq!(match_pattern("@app/common", "@app/common"), Some(""));
        assert_eq!(match_pattern("@app/*", "@lib/x"), None);
    }
}
