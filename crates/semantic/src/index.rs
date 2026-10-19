//! Project-wide symbol index: name lookups, module resolution, type resolution,
//! inheritance and DI maps.

use std::collections::{HashMap, HashSet};

use codegraph_core::extract::clean_type;
use codegraph_core::{DiRegistration, Import, ImportKind, Language, Symbol, SymbolKind};

use crate::Confidence;

#[derive(Clone, Debug)]
pub struct IndexedSymbol {
    pub file: String,
    pub lang: Language,
    pub sym: Symbol,
}

#[derive(Clone, Debug)]
pub struct DiEntry {
    pub file: String,
    pub family: &'static str,
    pub reg: DiRegistration,
}

/// Result of resolving a name to a symbol.
#[derive(Clone, Debug)]
pub enum Resolved {
    One(usize, Confidence, String),
    Many(Vec<usize>),
    None,
}

pub struct SymbolIndex {
    pub syms: Vec<IndexedSymbol>,
    pub by_id: HashMap<String, usize>,
    pub by_name: HashMap<String, Vec<usize>>,
    pub children: HashMap<String, Vec<usize>>,
    pub by_file: HashMap<String, Vec<usize>>,
    pub module_of_file: HashMap<String, usize>,
    pub file_lang: HashMap<String, Language>,
    /// Python dotted module path → file.
    pub py_modules: HashMap<String, String>,
    /// JS/TS file path without extension → file.
    pub js_paths: HashMap<String, String>,
    pub imports: HashMap<String, Vec<Import>>,
    pub file_namespace: HashMap<String, String>,
    /// type id → resolved base type ids (with confidence).
    pub bases: HashMap<String, Vec<(String, Confidence)>>,
    /// type id → subtypes / implementations.
    pub impls: HashMap<String, Vec<String>>,
    /// (family, service name/token) → registrations.
    pub di: HashMap<(String, String), Vec<DiEntry>>,
}

const JS_EXTS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"];

fn strip_js_ext(path: &str) -> &str {
    for e in JS_EXTS {
        if let Some(p) = path.strip_suffix(e) {
            return p;
        }
    }
    path
}

pub fn dirname(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

pub fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

impl SymbolIndex {
    pub fn build(syms: Vec<IndexedSymbol>, imports: HashMap<String, Vec<Import>>, di: Vec<(String, DiRegistration)>) -> SymbolIndex {
        let mut idx = SymbolIndex {
            syms,
            by_id: HashMap::new(),
            by_name: HashMap::new(),
            children: HashMap::new(),
            by_file: HashMap::new(),
            module_of_file: HashMap::new(),
            file_lang: HashMap::new(),
            py_modules: HashMap::new(),
            js_paths: HashMap::new(),
            imports,
            file_namespace: HashMap::new(),
            bases: HashMap::new(),
            impls: HashMap::new(),
            di: HashMap::new(),
        };
        for (i, s) in idx.syms.iter().enumerate() {
            idx.by_id.insert(s.sym.id.clone(), i);
            idx.by_file.entry(s.file.clone()).or_default().push(i);
            if s.sym.kind == SymbolKind::Module {
                idx.module_of_file.insert(s.file.clone(), i);
                idx.file_lang.insert(s.file.clone(), s.lang);
                match s.lang {
                    Language::Python => {
                        idx.py_modules.insert(s.sym.qualified_name.clone(), s.file.clone());
                    }
                    Language::TypeScript | Language::Tsx | Language::JavaScript => {
                        idx.js_paths.insert(strip_js_ext(&s.file).to_string(), s.file.clone());
                    }
                    _ => {}
                }
                continue;
            }
            idx.by_name.entry(s.sym.name.clone()).or_default().push(i);
            if let Some(p) = &s.sym.parent_id {
                idx.children.entry(p.clone()).or_default().push(i);
            }
            if let Some(ns) = &s.sym.namespace {
                idx.file_namespace.entry(s.file.clone()).or_insert_with(|| ns.clone());
            }
        }
        for (file, reg) in di {
            let fam = idx.file_lang.get(&file).map(|l| l.family()).unwrap_or("");
            let key = (fam.to_string(), clean_type(&reg.service));
            idx.di.entry(key).or_default().push(DiEntry { file, family: fam, reg });
        }
        // Inheritance: resolve every declared base.
        let type_ids: Vec<usize> = (0..idx.syms.len()).filter(|i| idx.syms[*i].sym.kind.is_type()).collect();
        for i in type_ids {
            let s = &idx.syms[i];
            let file = s.file.clone();
            let id = s.sym.id.clone();
            let bases = s.sym.bases.clone();
            let mut resolved = Vec::new();
            for b in bases {
                if let Resolved::One(t, c, _) = idx.resolve_type(&b, &file) {
                    let tid = idx.syms[t].sym.id.clone();
                    if tid != id {
                        resolved.push((tid, c));
                    }
                }
            }
            for (tid, _) in &resolved {
                idx.impls.entry(tid.clone()).or_default().push(id.clone());
            }
            idx.bases.insert(id, resolved);
        }
        idx
    }

    pub fn get(&self, id: &str) -> Option<&IndexedSymbol> {
        self.by_id.get(id).map(|i| &self.syms[*i])
    }

    pub fn family_of_file(&self, file: &str) -> &'static str {
        self.file_lang.get(file).map(|l| l.family()).unwrap_or("")
    }

    fn same_family(&self, i: usize, family: &str) -> bool {
        self.syms[i].lang.family() == family
    }

    /// Nearest enclosing type of a symbol (itself if it is a type).
    pub fn enclosing_type(&self, id: &str) -> Option<usize> {
        let mut cur = self.by_id.get(id).copied();
        let mut guard = 0;
        while let Some(i) = cur {
            if self.syms[i].sym.kind.is_type() {
                return Some(i);
            }
            cur = self.syms[i].sym.parent_id.as_ref().and_then(|p| self.by_id.get(p).copied());
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        None
    }

    /// Members named `name` declared in the type or inherited from its bases.
    pub fn members_named(&self, type_idx: usize, name: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![(type_idx, 0)];
        while let Some((t, depth)) = stack.pop() {
            let tid = &self.syms[t].sym.id;
            if !seen.insert(tid.clone()) {
                continue;
            }
            if let Some(kids) = self.children.get(tid) {
                let found: Vec<usize> = kids.iter().copied().filter(|k| self.syms[*k].sym.name == name).collect();
                if !found.is_empty() {
                    out.extend(found);
                    continue; // nearest declaration wins (override)
                }
            }
            if depth < 6 {
                if let Some(bs) = self.bases.get(tid) {
                    for (b, _) in bs {
                        if let Some(bi) = self.by_id.get(b) {
                            stack.push((*bi, depth + 1));
                        }
                    }
                }
            }
        }
        out
    }

    /// All (transitive) implementations / subclasses of a type.
    pub fn implementations(&self, type_id: &str) -> Vec<usize> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![type_id.to_string()];
        while let Some(t) = stack.pop() {
            if let Some(subs) = self.impls.get(&t) {
                for s in subs {
                    if seen.insert(s.clone()) {
                        if let Some(i) = self.by_id.get(s) {
                            out.push(*i);
                        }
                        stack.push(s.clone());
                    }
                }
            }
        }
        out
    }

    pub fn is_abstract_type(&self, i: usize) -> bool {
        let s = &self.syms[i].sym;
        s.kind == SymbolKind::Interface || s.is_abstract
    }

    // -----------------------------------------------------------------------
    // Module resolution
    // -----------------------------------------------------------------------

    /// Resolves a JS/TS module specifier to an indexed file.
    pub fn resolve_js_module(&self, from_file: &str, spec: &str) -> Option<(String, Confidence)> {
        let try_base = |base: &str| -> Option<String> {
            let base = normalize_path(base);
            if let Some(f) = self.js_paths.get(strip_js_ext(&base)) {
                return Some(f.clone());
            }
            if let Some(f) = self.js_paths.get(&format!("{base}/index")) {
                return Some(f.clone());
            }
            None
        };
        if spec.starts_with('.') {
            let base = format!("{}/{}", dirname(from_file), spec);
            return try_base(&base).map(|f| (f, Confidence::Exact));
        }
        if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
            // Common tsconfig alias: "@/x" → "<project>/src/x"
            let mut dir = dirname(from_file).to_string();
            loop {
                let cand = if dir.is_empty() { format!("src/{rest}") } else { format!("{dir}/src/{rest}") };
                if let Some(f) = try_base(&cand) {
                    return Some((f, Confidence::Exact));
                }
                if dir.is_empty() {
                    break;
                }
                dir = dirname(&dir).to_string();
            }
        }
        if spec.contains('/') && !spec.starts_with('@') {
            // Path alias (tsconfig paths / baseUrl): unique suffix match.
            let needle = format!("/{}", spec.trim_start_matches('/'));
            let hits: Vec<&String> = self
                .js_paths
                .iter()
                .filter(|(k, _)| k.ends_with(&needle) || k.ends_with(&format!("{needle}/index")))
                .map(|(_, v)| v)
                .collect();
            if hits.len() == 1 {
                return Some((hits[0].clone(), Confidence::Inferred));
            }
        }
        None
    }

    /// Resolves a Python module path (`app.services`, `.models`, `..x`) to a file.
    pub fn resolve_py_module(&self, from_file: &str, module: &str) -> Option<(String, Confidence)> {
        if module.starts_with('.') {
            let dots = module.chars().take_while(|c| *c == '.').count();
            let rest = &module[dots..];
            let mut dir = dirname(from_file).to_string();
            for _ in 1..dots {
                dir = dirname(&dir).to_string();
            }
            let base = if dir.is_empty() { String::new() } else { format!("{}.", dir.replace('/', ".")) };
            let full = format!("{base}{rest}");
            let full = full.trim_end_matches('.');
            return self.py_modules.get(full).map(|f| (f.clone(), Confidence::Exact));
        }
        if let Some(f) = self.py_modules.get(module) {
            return Some((f.clone(), Confidence::Exact));
        }
        let needle = format!(".{module}");
        let hits: Vec<(&String, &String)> = self.py_modules.iter().filter(|(k, _)| k.ends_with(&needle)).collect();
        match hits.len() {
            0 => None,
            1 => Some((hits[0].1.clone(), Confidence::Exact)),
            _ => {
                // Prefer the candidate sharing the longest directory prefix with the importer.
                let best = hits.iter().max_by_key(|(_, f)| common_prefix_len(f, from_file)).map(|(_, f)| (*f).clone())?;
                Some((best, Confidence::Inferred))
            }
        }
    }

    pub fn resolve_module(&self, from_file: &str, spec: &str) -> Option<(String, Confidence)> {
        match self.file_lang.get(from_file) {
            Some(Language::Python) => self.resolve_py_module(from_file, spec),
            Some(Language::TypeScript) | Some(Language::Tsx) | Some(Language::JavaScript) => self.resolve_js_module(from_file, spec),
            _ => None,
        }
    }

    /// Top-level symbol exported by `file` under `name` (follows re-exports).
    pub fn exported_symbol(&self, file: &str, name: &str, depth: u32) -> Option<usize> {
        let module_id = self.module_of_file.get(file).map(|m| self.syms[*m].sym.id.clone())?;
        let kids = self.children.get(&module_id);
        if let Some(kids) = kids {
            if name == "default" {
                if let Some(k) = kids.iter().find(|k| self.syms[**k].sym.attrs.iter().any(|a| a == "export default")) {
                    return Some(*k);
                }
            }
            if let Some(k) = kids.iter().find(|k| self.syms[**k].sym.name == name) {
                return Some(*k);
            }
        }
        if depth < 4 {
            if let Some(imps) = self.imports.get(file) {
                for imp in imps {
                    let binds = imp.local.as_deref() == Some(name) || (imp.kind == ImportKind::Namespace && imp.local.is_none());
                    if binds {
                        if let Some((f, _)) = self.resolve_module(file, &imp.module) {
                            let inner = if imp.local.is_none() { name } else { imp.imported.as_deref().unwrap_or(name) };
                            if let Some(s) = self.exported_symbol(&f, inner, depth + 1) {
                                return Some(s);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Finds the import that binds `local` in `file`.
    pub fn import_binding(&self, file: &str, local: &str) -> Option<&Import> {
        self.imports.get(file)?.iter().find(|i| i.local.as_deref() == Some(local))
    }

    /// Resolves an imported local name to a symbol (or module node for namespace imports).
    pub fn resolve_imported(&self, file: &str, local: &str) -> Option<(usize, Confidence, String)> {
        let imp = self.import_binding(file, local)?;
        let (target_file, conf) = self.resolve_module(file, &imp.module)?;
        match imp.kind {
            ImportKind::Named | ImportKind::Default => {
                let name = imp.imported.as_deref().unwrap_or(local);
                let s = self.exported_symbol(&target_file, name, 0)?;
                Some((s, conf, format!("direct import from '{}'", imp.module)))
            }
            _ => {
                // Python `from pkg import module` binds a module.
                let m = *self.module_of_file.get(&target_file)?;
                Some((m, conf, format!("module import '{}'", imp.module)))
            }
        }
    }

    // -----------------------------------------------------------------------
    // Type resolution
    // -----------------------------------------------------------------------

    pub fn resolve_type(&self, raw: &str, from_file: &str) -> Resolved {
        let cleaned = clean_type(raw);
        if cleaned.is_empty() {
            return Resolved::None;
        }
        let family = self.family_of_file(from_file);
        // Qualified names: `Shop.Models.Order`, `models.Order`, `ns.Type`.
        if cleaned.contains('.') {
            let exact: Vec<usize> = self
                .syms
                .iter()
                .enumerate()
                .filter(|(_, s)| s.sym.kind.is_type() && s.sym.qualified_name == cleaned)
                .map(|(i, _)| i)
                .collect();
            if exact.len() == 1 {
                return Resolved::One(exact[0], Confidence::Exact, format!("qualified type name '{cleaned}'"));
            }
            let (head, last) = cleaned.rsplit_once('.').unwrap();
            if let Some((m, conf, ev)) = self.resolve_imported(from_file, head.split('.').next().unwrap_or(head)) {
                if self.syms[m].sym.kind == SymbolKind::Module {
                    if let Some(s) = self.exported_symbol(&self.syms[m].file, last, 0) {
                        return Resolved::One(s, conf, ev);
                    }
                }
            }
            return self.resolve_type(last, from_file);
        }
        let name = cleaned.as_str();
        let cands: Vec<usize> = self
            .by_name
            .get(name)
            .map(|v| v.iter().copied().filter(|i| self.syms[*i].sym.kind.is_type() && self.same_family(*i, family)).collect())
            .unwrap_or_default();
        // Imports first (JS / Python): an explicit binding beats everything.
        if let Some((s, conf, ev)) = self.resolve_imported(from_file, name) {
            if self.syms[s].sym.kind.is_type() {
                return Resolved::One(s, conf, ev);
            }
        }
        if cands.is_empty() {
            return Resolved::None;
        }
        let same_file: Vec<usize> = cands.iter().copied().filter(|i| self.syms[*i].file == from_file).collect();
        if same_file.len() == 1 {
            return Resolved::One(same_file[0], Confidence::Exact, "same-file declaration".into());
        }
        if family == "dotnet" {
            let mut visible_ns: Vec<String> = Vec::new();
            if let Some(ns) = self.file_namespace.get(from_file) {
                // A namespace sees its own types and its parents'.
                let mut cur = ns.as_str();
                loop {
                    visible_ns.push(cur.to_string());
                    match cur.rfind('.') {
                        Some(i) => cur = &cur[..i],
                        None => break,
                    }
                }
            }
            let mut usings: Vec<String> = Vec::new();
            if let Some(imps) = self.imports.get(from_file) {
                for i in imps {
                    if i.kind == ImportKind::Using {
                        if let Some(alias) = &i.local {
                            if alias == name {
                                return self.resolve_type(&i.module, from_file);
                            }
                        } else {
                            usings.push(i.module.clone());
                        }
                    }
                }
            }
            let visible: Vec<usize> = cands
                .iter()
                .copied()
                .filter(|i| {
                    let ns = self.syms[*i].sym.namespace.clone().unwrap_or_default();
                    visible_ns.contains(&ns) || usings.contains(&ns) || ns.is_empty()
                })
                .collect();
            if visible.len() == 1 {
                let ns = self.syms[visible[0]].sym.namespace.clone().unwrap_or_default();
                let how = if usings.contains(&ns) {
                    format!("using {ns}")
                } else if ns.is_empty() {
                    "global namespace".to_string()
                } else {
                    format!("namespace {ns}")
                };
                return Resolved::One(visible[0], Confidence::Exact, format!("type match via {how}"));
            }
            if visible.len() > 1 {
                return Resolved::Many(visible);
            }
        }
        if cands.len() == 1 {
            return Resolved::One(cands[0], Confidence::Inferred, format!("unique type name '{name}'"));
        }
        Resolved::Many(cands)
    }

    /// Resolves a value-level name (function, class, variable) visible from `file`.
    pub fn resolve_value(&self, name: &str, from_file: &str, enclosing: &str) -> Resolved {
        // Same file, innermost scope first.
        let mut scope = Some(enclosing.to_string());
        let mut guard = 0;
        while let Some(sid) = scope {
            // Class members are never visible as bare names here (C# implicit `this`
            // is resolved by the call resolver / extractor instead).
            let scope_is_type = self.get(&sid).map(|p| p.sym.kind.is_type()).unwrap_or(false);
            if let (false, Some(kids)) = (scope_is_type, self.children.get(&sid)) {
                let hit = kids.iter().find(|k| {
                    let s = &self.syms[**k].sym;
                    s.name == name
                        && !matches!(
                            s.kind,
                            SymbolKind::Method | SymbolKind::Constructor | SymbolKind::Property | SymbolKind::Field | SymbolKind::Test
                        )
                });
                if let Some(k) = hit {
                    return Resolved::One(*k, Confidence::Exact, "same-file declaration".into());
                }
            }
            scope = self.get(&sid).and_then(|s| s.sym.parent_id.clone());
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        if let Some((s, c, ev)) = self.resolve_imported(from_file, name) {
            return Resolved::One(s, c, ev);
        }
        Resolved::None
    }

    pub fn coords(&self, i: usize) -> String {
        let s = &self.syms[i];
        format!("{}:{}", s.file, s.sym.start_line)
    }
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}
