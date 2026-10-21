//! `vite.config.*` / `vitest.config.*`: the module aliases (`resolve.alias`) and the dev-server
//! proxies (`server.proxy`) that decide how imports and frontend HTTP calls resolve.
//!
//! Only literal configuration is understood (objects, arrays, `path.resolve(__dirname, 'src')`,
//! `fileURLToPath(new URL('./src', import.meta.url))`); values computed at runtime are skipped.

use tree_sitter::Node;

use crate::lang::{parse, Language};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViteConfig {
    /// Alias → target directory or file, relative to the config's directory.
    pub aliases: Vec<(String, String)>,
    pub proxies: Vec<ViteProxy>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViteProxy {
    /// Path prefix forwarded to the backend (`/api`).
    pub prefix: String,
    /// The proxy's `rewrite` removes the prefix: `/api/users` reaches the backend as `/users`.
    pub strip: bool,
}

pub fn is_vite_config(file_name: &str) -> bool {
    let n = file_name.to_ascii_lowercase();
    (n.starts_with("vite.config.") || n.starts_with("vitest.config."))
        && [".ts", ".js", ".mjs", ".mts", ".cjs", ".cts"].iter().any(|e| n.ends_with(e))
}

pub fn parse_vite_config(src: &str) -> ViteConfig {
    let mut out = ViteConfig::default();
    let Some(tree) = parse(Language::TypeScript, src) else { return out };
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind() == "pair" {
            match key_text(n, src).as_deref() {
                Some("alias") => {
                    if let Some(v) = n.child_by_field_name("value") {
                        aliases(v, src, &mut out.aliases);
                    }
                    continue;
                }
                Some("proxy") => {
                    if let Some(v) = n.child_by_field_name("value") {
                        proxies(v, src, &mut out.proxies);
                    }
                    continue;
                }
                _ => {}
            }
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
    out
}

fn text<'s>(n: Node, src: &'s str) -> &'s str {
    &src[n.byte_range()]
}

fn key_text(pair: Node, src: &str) -> Option<String> {
    pair.child_by_field_name("key").map(|k| text(k, src).trim_matches(['"', '\'', '`']).to_string())
}

fn string_value(n: Node, src: &str) -> Option<String> {
    match n.kind() {
        "string" => Some(text(n, src).trim_matches(['"', '\'']).to_string()),
        "template_string" if !text(n, src).contains("${") => Some(text(n, src).trim_matches('`').to_string()),
        _ => None,
    }
}

/// A path expression: a string, or the string arguments of `path.resolve(…)` / `new URL(…)`
/// joined (`__dirname`, `process.cwd()` and `import.meta.url` are the config directory).
fn path_value(n: Node, src: &str) -> Option<String> {
    if let Some(s) = string_value(n, src) {
        return Some(normalize(&s));
    }
    let mut parts = Vec::new();
    let mut stack = vec![n];
    while let Some(x) = stack.pop() {
        if let Some(s) = string_value(x, src) {
            parts.push(s);
            continue;
        }
        let mut c = x.walk();
        let kids: Vec<Node> = x.named_children(&mut c).collect();
        stack.extend(kids.into_iter().rev());
    }
    (!parts.is_empty()).then(|| normalize(&parts.join("/")))
}

fn normalize(p: &str) -> String {
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    segs.join("/")
}

fn aliases(v: Node, src: &str, out: &mut Vec<(String, String)>) {
    let mut c = v.walk();
    match v.kind() {
        // `alias: { '@': path.resolve(__dirname, 'src') }`
        "object" => {
            for p in v.named_children(&mut c).filter(|p| p.kind() == "pair") {
                if let (Some(k), Some(t)) = (key_text(p, src), p.child_by_field_name("value").and_then(|x| path_value(x, src))) {
                    out.push((k, t));
                }
            }
        }
        // `alias: [{ find: '@', replacement: … }]` (regex `find`s are skipped).
        "array" => {
            for o in v.named_children(&mut c).filter(|o| o.kind() == "object") {
                let mut oc = o.walk();
                let pairs: Vec<Node> = o.named_children(&mut oc).filter(|p| p.kind() == "pair").collect();
                let get = |key: &str| pairs.iter().find(|p| key_text(**p, src).as_deref() == Some(key)).and_then(|p| p.child_by_field_name("value"));
                let find = get("find").and_then(|f| string_value(f, src));
                let repl = get("replacement").and_then(|r| path_value(r, src));
                if let (Some(f), Some(r)) = (find, repl) {
                    out.push((f, r));
                }
            }
        }
        _ => {}
    }
}

fn proxies(v: Node, src: &str, out: &mut Vec<ViteProxy>) {
    if v.kind() != "object" {
        return;
    }
    let mut c = v.walk();
    for p in v.named_children(&mut c).filter(|p| p.kind() == "pair") {
        let Some(prefix) = key_text(p, src).filter(|k| k.starts_with('/')) else { continue };
        // `rewrite: (path) => path.replace(/^\/api/, '')`
        let strip = p
            .child_by_field_name("value")
            .filter(|x| x.kind() == "object")
            .map(|o| {
                let t = text(o, src);
                t.contains("rewrite") && t.contains("replace") && (t.contains("''") || t.contains("\"\"") || t.contains("``"))
            })
            .unwrap_or(false);
        out.push(ViteProxy { prefix, strip });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_and_proxies() {
        let cfg = parse_vite_config(
            "import { fileURLToPath, URL } from 'node:url';\n\
             export default defineConfig({\n\
               resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)), '~ui': path.resolve(__dirname, 'src', 'ui') } },\n\
               server: { proxy: { '/api': { target: 'http://localhost:3000', rewrite: (p) => p.replace(/^\\/api/, '') }, '/ws': 'ws://x' } },\n\
             });",
        );
        assert_eq!(cfg.aliases, vec![("@".to_string(), "src".to_string()), ("~ui".to_string(), "src/ui".to_string())]);
        assert_eq!(cfg.proxies, vec![ViteProxy { prefix: "/api".into(), strip: true }, ViteProxy { prefix: "/ws".into(), strip: false }]);
        let arr = parse_vite_config("export default { resolve: { alias: [{ find: '@lib', replacement: '/lib' }, { find: /^~/, replacement: '' }] } }");
        assert_eq!(arr.aliases, vec![("@lib".to_string(), "lib".to_string())]);
        assert!(is_vite_config("vite.config.mts") && !is_vite_config("vite.config.json"));
    }
}
