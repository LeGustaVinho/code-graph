//! Architecture guard: layer classification and dependency rules.
//!
//! Rules come from `.codegraph/architecture.toml` (or `codegraph.toml` with an
//! `[architecture]` table). Without configuration, a conventional layered
//! architecture is assumed based on path segments.
//!
//! ```toml
//! [[layers]]
//! name = "domain"
//! paths = ["src/Domain/**", "**/models/**"]
//!
//! [[rules]]
//! from = "domain"
//! deny = ["infrastructure", "presentation"]
//! ```

use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct LayerDef {
    pub name: String,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RuleDef {
    pub from: String,
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub deny: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct ArchConfigFile {
    #[serde(default)]
    pub layers: Vec<LayerDef>,
    #[serde(default)]
    pub rules: Vec<RuleDef>,
}

#[derive(Deserialize)]
struct Wrapper {
    architecture: Option<ArchConfigFile>,
}

pub struct ArchGuard {
    layers: Vec<(String, GlobSet)>,
    rules: Vec<RuleDef>,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct Verdict {
    pub allowed: bool,
    pub from_layer: Option<String>,
    pub to_layer: Option<String>,
    pub rule: String,
}

const DEFAULT_LAYERS: &[(&str, &[&str])] = &[
    ("tests", &["tests", "test", "__tests__", "spec", "specs"]),
    (
        "presentation",
        &["controllers", "controller", "api", "routes", "pages", "components", "views", "ui", "endpoints", "frontend", "web", "app"],
    ),
    ("application", &["services", "service", "application", "usecases", "use_cases", "handlers", "commands", "queries"]),
    ("domain", &["domain", "entities", "entity", "models", "model", "core"]),
    (
        "infrastructure",
        &["infrastructure", "infra", "data", "repositories", "repository", "persistence", "db", "database", "migrations", "prisma"],
    ),
];

fn default_rules() -> Vec<RuleDef> {
    let r =
        |from: &str, deny: &[&str]| RuleDef { from: from.into(), allow: None, deny: Some(deny.iter().map(|s| s.to_string()).collect()) };
    vec![
        r("domain", &["presentation", "application", "infrastructure"]),
        r("application", &["presentation"]),
        r("infrastructure", &["presentation"]),
        r("presentation", &["infrastructure"]),
    ]
}

impl ArchGuard {
    pub fn load(root: &Path) -> ArchGuard {
        let candidates = [root.join(".codegraph").join("architecture.toml"), root.join("codegraph.toml")];
        for c in candidates {
            if let Ok(text) = std::fs::read_to_string(&c) {
                let parsed: Option<ArchConfigFile> = toml::from_str::<ArchConfigFile>(&text)
                    .ok()
                    .filter(|f| !f.layers.is_empty())
                    .or_else(|| toml::from_str::<Wrapper>(&text).ok().and_then(|w| w.architecture));
                if let Some(cfg) = parsed {
                    return ArchGuard::from_config(cfg, c.display().to_string());
                }
            }
        }
        ArchGuard {
            layers: Vec::new(),
            rules: default_rules(),
            source: "default layered convention (no .codegraph/architecture.toml)".into(),
        }
    }

    pub fn from_config(cfg: ArchConfigFile, source: String) -> ArchGuard {
        let mut layers = Vec::new();
        for l in cfg.layers {
            let mut b = GlobSetBuilder::new();
            for p in &l.paths {
                if let Ok(g) = Glob::new(p) {
                    b.add(g);
                }
            }
            if let Ok(set) = b.build() {
                layers.push((l.name, set));
            }
        }
        ArchGuard { layers, rules: cfg.rules, source }
    }

    /// Layer of a file path or module name.
    pub fn classify(&self, path: &str) -> Option<String> {
        let p = path.replace('\\', "/");
        if !self.layers.is_empty() {
            for (name, set) in &self.layers {
                if set.is_match(&p) {
                    return Some(name.clone());
                }
            }
            // Bare layer names are accepted as-is.
            if self.layers.iter().any(|(n, _)| n.eq_ignore_ascii_case(&p)) {
                return Some(p.to_ascii_lowercase());
            }
            return None;
        }
        if DEFAULT_LAYERS.iter().any(|(n, _)| n.eq_ignore_ascii_case(&p)) {
            return Some(p.to_ascii_lowercase());
        }
        // Default: test folders win, otherwise the deepest matching path segment.
        let segs: Vec<String> = p.to_ascii_lowercase().split(['/', '.']).map(|s| s.to_string()).collect();
        let mut best: Option<&str> = None;
        for seg in &segs {
            for (layer, keys) in DEFAULT_LAYERS {
                if keys.contains(&seg.as_str()) {
                    if *layer == "tests" {
                        return Some("tests".into());
                    }
                    best = Some(layer);
                }
            }
        }
        best.map(|b| b.to_string())
    }

    pub fn check(&self, from: &str, to: &str) -> Verdict {
        let fl = self.classify(from);
        let tl = self.classify(to);
        let (Some(f), Some(t)) = (fl.clone(), tl.clone()) else {
            return Verdict { allowed: true, from_layer: fl, to_layer: tl, rule: "no layer matched: unconstrained".into() };
        };
        if f == t {
            return Verdict { allowed: true, from_layer: fl, to_layer: tl, rule: format!("same layer '{f}'") };
        }
        if f == "tests" {
            return Verdict { allowed: true, from_layer: fl, to_layer: tl, rule: "tests may depend on any layer".into() };
        }
        for r in self.rules.iter().filter(|r| r.from == f) {
            if let Some(deny) = &r.deny {
                if deny.contains(&t) {
                    return Verdict {
                        allowed: false,
                        from_layer: fl,
                        to_layer: tl,
                        rule: format!("rule: '{f}' must not depend on '{t}' ({})", self.source),
                    };
                }
            }
            if let Some(allow) = &r.allow {
                if !allow.contains(&t) {
                    return Verdict {
                        allowed: false,
                        from_layer: fl,
                        to_layer: tl,
                        rule: format!("rule: '{f}' may only depend on [{}] ({})", allow.join(", "), self.source),
                    };
                }
            }
        }
        Verdict { allowed: true, from_layer: fl, to_layer: tl, rule: format!("no rule forbids '{f}' → '{t}'") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_convention() {
        let g = ArchGuard { layers: Vec::new(), rules: default_rules(), source: "default".into() };
        assert_eq!(g.classify("src/Domain/Order.cs").as_deref(), Some("domain"));
        assert_eq!(g.classify("backend/Controllers/OrdersController.cs").as_deref(), Some("presentation"));
        assert!(!g.check("src/domain/order.ts", "src/infrastructure/db.ts").allowed);
        assert!(g.check("src/services/x.ts", "src/domain/y.ts").allowed);
        assert!(!g.check("src/Controllers/A.cs", "src/Repositories/B.cs").allowed);
    }

    #[test]
    fn configured_layers() {
        let cfg: ArchConfigFile = toml::from_str(
            r#"
            [[layers]]
            name = "core"
            paths = ["core/**"]
            [[layers]]
            name = "adapters"
            paths = ["adapters/**"]
            [[rules]]
            from = "core"
            allow = []
            "#,
        )
        .unwrap();
        let g = ArchGuard::from_config(cfg, "test".into());
        assert!(!g.check("core/a.py", "adapters/b.py").allowed);
        assert!(g.check("adapters/b.py", "core/a.py").allowed);
    }
}
