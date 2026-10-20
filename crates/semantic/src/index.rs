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
    /// Every namespace declared in a file (a file may hold several namespace blocks).
    pub file_namespace: HashMap<String, Vec<String>>,
    /// type id → resolved base type ids (with confidence).
    pub bases: HashMap<String, Vec<(String, Confidence)>>,
    /// type id → subtypes / implementations.
    pub impls: HashMap<String, Vec<String>>,
    /// (family, service name/token) → registrations.
    pub di: HashMap<(String, String), Vec<DiEntry>>,
    /// Type index → every declaration of the same logical type (C# `partial` parts,
    /// TS interface merging), the type itself included. Absent for ordinary types.
    pub parts: HashMap<usize, Vec<usize>>,
    /// C# `global using` namespaces (visible from every file).
    pub global_usings: Vec<String>,
    /// Extension method name → (method index, extended type name).
    pub extensions: HashMap<String, Vec<(usize, String)>>,
    /// tsconfig / jsconfig paths and workspace packages (set by the indexer, which reads them).
    pub js: crate::jsconfig::JsProject,
    /// Unity: directories of Editor-only assembly definitions (set by the indexer).
    pub editor_dirs: Vec<String>,
    /// Unity: the C# files that are test code (`unity::apply_test_files`); None elsewhere.
    pub unity_test_files: Option<HashSet<String>>,
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
    /// Builds the index. Symbols and DI registrations are put in a canonical order first, so
    /// every "first match" below is independent of how (or in which row order) they were loaded.
    pub fn build(
        mut syms: Vec<IndexedSymbol>,
        imports: HashMap<String, Vec<Import>>,
        mut di: Vec<(String, DiRegistration)>,
    ) -> SymbolIndex {
        syms.sort_by(|a, b| (&a.file, a.sym.start_line, &a.sym.id).cmp(&(&b.file, b.sym.start_line, &b.sym.id)));
        di.sort_by(|a, b| (&a.0, a.1.line).cmp(&(&b.0, b.1.line)));
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
            parts: HashMap::new(),
            global_usings: Vec::new(),
            extensions: HashMap::new(),
            js: Default::default(),
            editor_dirs: Vec::new(),
            unity_test_files: None,
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
                let list = idx.file_namespace.entry(s.file.clone()).or_default();
                if !list.contains(ns) {
                    list.push(ns.clone());
                }
            }
        }
        // Logical types declared in several parts.
        let mut groups: HashMap<(String, String, SymbolKind), Vec<usize>> = HashMap::new();
        for (i, s) in idx.syms.iter().enumerate() {
            if !s.sym.kind.is_type() {
                continue;
            }
            let key = if s.lang.family() == "dotnet" && s.sym.modifiers.iter().any(|m| m == "partial") {
                ("dotnet".to_string(), s.sym.qualified_name.clone(), s.sym.kind)
            } else if s.sym.kind == SymbolKind::Interface {
                // TS declaration merging: same interface declared twice in one module.
                (format!("file:{}", s.file), s.sym.qualified_name.clone(), s.sym.kind)
            } else {
                continue;
            };
            groups.entry(key).or_default().push(i);
        }
        for (_, g) in groups {
            if g.len() > 1 {
                for i in &g {
                    idx.parts.insert(*i, g.clone());
                }
            }
        }
        for (i, s) in idx.syms.iter().enumerate() {
            if let Some(t) = s.sym.modifiers.iter().find_map(|m| m.strip_prefix("extension:")) {
                idx.extensions.entry(s.sym.name.clone()).or_default().push((i, t.to_string()));
            }
        }
        for imps in idx.imports.values() {
            for i in imps {
                if i.kind == ImportKind::GlobalUsing && !idx.global_usings.contains(&i.module) {
                    idx.global_usings.push(i.module.clone());
                }
            }
        }
        idx.global_usings.sort();
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

    /// Sorts symbol indices canonically (file, start line, id): `syms` is kept in that order.
    pub fn sort_canonical(&self, v: &mut [usize]) {
        v.sort_unstable();
    }

    /// Canonical first of several candidate symbols.
    pub fn canonical_first(&self, v: &[usize]) -> Option<usize> {
        v.iter().copied().min()
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
            // Partial declarations contribute members (and bases) to the same type.
            for p in self.parts_of(t) {
                if p != t {
                    stack.push((p, depth));
                }
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

    /// Unity editor-only code: an `Editor` folder anywhere in the path, or an assembly
    /// definition restricted to the Editor platform.
    pub fn is_editor_file(&self, file: &str) -> bool {
        file.split('/').rev().skip(1).any(|seg| seg == "Editor")
            || self.editor_dirs.iter().any(|d| !d.is_empty() && file.starts_with(&format!("{d}/")))
    }

    /// Is `file` test code? Unity C# follows the test assemblies; everything else the
    /// file-name / folder conventions.
    pub fn is_test_file(&self, file: &str) -> bool {
        match &self.unity_test_files {
            Some(tests) if self.file_lang.get(file) == Some(&Language::CSharp) => tests.contains(file),
            _ => codegraph_core::extract::is_test_path(file),
        }
    }

    /// Every declaration of the logical type `t` (just `t` unless it is partial / merged).
    pub fn parts_of(&self, t: usize) -> Vec<usize> {
        self.parts.get(&t).cloned().unwrap_or_else(|| vec![t])
    }

    /// Canonical declaration standing for the logical type `t`.
    pub fn representative(&self, t: usize) -> usize {
        self.parts.get(&t).and_then(|p| p.iter().copied().min()).unwrap_or(t)
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
        // tsconfig / jsconfig `paths` and `baseUrl` of the config governing this file.
        if let Some(cfg) = self.js.nearest(from_file) {
            for (pattern, targets) in &cfg.paths {
                if let Some(star) = crate::jsconfig::match_pattern(pattern, spec) {
                    for t in targets {
                        if let Some(f) = try_base(&t.replacen('*', star, 1)) {
                            return Some((f, Confidence::Exact));
                        }
                    }
                }
            }
            if let Some(base) = &cfg.base_url {
                let cand = if base.is_empty() { spec.to_string() } else { format!("{base}/{spec}") };
                if let Some(f) = try_base(&cand) {
                    return Some((f, Confidence::Exact));
                }
            }
        }
        // Workspace packages importing each other by name (`@acme/shared`, `@acme/shared/x`).
        for pkg in &self.js.packages {
            let rest = if spec == pkg.name { Some("") } else { spec.strip_prefix(&format!("{}/", pkg.name)) };
            let Some(rest) = rest else { continue };
            let join = |p: &str| if pkg.dir.is_empty() { p.to_string() } else { format!("{}/{p}", pkg.dir) };
            let cands: Vec<String> = if rest.is_empty() {
                pkg.entries.iter().map(|e| join(e)).chain([join("src/index"), join("index")]).collect()
            } else {
                vec![join(rest), join(&format!("src/{rest}"))]
            };
            for c in cands {
                if let Some(f) = try_base(&c) {
                    return Some((f, Confidence::Exact));
                }
            }
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
                // Prefer the candidate sharing the longest prefix with the importer; ties go to
                // the lexicographically smallest path (never to HashMap iteration order).
                let best = hits
                    .iter()
                    .map(|(_, f)| *f)
                    .min_by(|a, b| common_prefix_len(b, from_file).cmp(&common_prefix_len(a, from_file)).then_with(|| a.cmp(b)))
                    .cloned()?;
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
            // `export { a as b }` / `exports.b = a`.
            let alias = format!("export-as:{name}");
            if let Some(k) = kids.iter().find(|k| self.syms[**k].sym.modifiers.contains(&alias)) {
                return Some(*k);
            }
        }
        if depth < 4 {
            if let Some(imps) = self.imports.get(file) {
                for imp in imps {
                    // `export * as ns from './x'`: `ns` is the module itself.
                    if imp.kind == ImportKind::Namespace && imp.local.as_deref() == Some(name) && imp.imported.is_none() {
                        if let Some((f, _)) = self.resolve_module(file, &imp.module) {
                            if let Some(m) = self.module_of_file.get(&f) {
                                return Some(*m);
                            }
                        }
                    }
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
        self.resolve_type_ns(raw, from_file, None)
    }

    /// Like [`resolve_type`], seen from namespace `ns` (C#: the namespace of the referencing
    /// symbol; files may hold several namespace blocks). `None` = every namespace of the file.
    pub fn resolve_type_ns(&self, raw: &str, from_file: &str, ns: Option<&str>) -> Resolved {
        self.resolve_type_inner(raw, from_file, ns, true)
    }

    /// `aliases = false` once a `using Alias = X.Y;` has been followed: `using Debug =
    /// UnityEngine.Debug;` would otherwise resolve `Debug` through itself forever.
    fn resolve_type_inner(&self, raw: &str, from_file: &str, ns: Option<&str>, aliases: bool) -> Resolved {
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
            return self.resolve_type_inner(last, from_file, ns, aliases);
        }
        let name = cleaned.as_str();
        let mut cands: Vec<usize> = self
            .by_name
            .get(name)
            .map(|v| v.iter().copied().filter(|i| self.syms[*i].sym.kind.is_type() && self.same_family(*i, family)).collect())
            .unwrap_or_default();
        // Parts of one partial / merged type are a single candidate.
        if !self.parts.is_empty() {
            cands = cands.into_iter().map(|c| self.representative(c)).collect();
            cands.sort_unstable();
            cands.dedup();
        }
        // Imports first (JS / Python): an explicit binding beats everything.
        if let Some((s, conf, ev)) = self.resolve_imported(from_file, name) {
            if self.syms[s].sym.kind.is_type() {
                return Resolved::One(s, conf, ev);
            }
        }
        // C# `using Alias = Some.Type;` names a type that has no declaration of its own.
        if family == "dotnet" && aliases {
            if let Some(target) = self.imports.get(from_file).and_then(|imps| {
                imps.iter().find(|i| i.kind == ImportKind::Using && i.local.as_deref() == Some(name)).map(|i| i.module.clone())
            }) {
                if target != name {
                    return self.resolve_type_inner(&target, from_file, ns, false);
                }
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
            let file_ns: Vec<String> = match ns {
                Some(n) => vec![n.to_string()],
                None => self.file_namespace.get(from_file).cloned().unwrap_or_default(),
            };
            for ns in &file_ns {
                // A namespace sees its own types and its parents'.
                let mut cur = ns.as_str();
                loop {
                    if !visible_ns.iter().any(|v| v == cur) {
                        visible_ns.push(cur.to_string());
                    }
                    match cur.rfind('.') {
                        Some(i) => cur = &cur[..i],
                        None => break,
                    }
                }
            }
            let mut usings: Vec<String> = self.global_usings.clone();
            if let Some(imps) = self.imports.get(from_file) {
                for i in imps {
                    if i.kind == ImportKind::Using {
                        if let Some(alias) = &i.local {
                            if alias == name {
                                if aliases {
                                    return self.resolve_type_inner(&i.module, from_file, ns, false);
                                }
                                continue;
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
