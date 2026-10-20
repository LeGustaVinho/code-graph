//! Project-wide symbol index: name lookups, module resolution, type resolution,
//! inheritance and DI maps.

use std::collections::{HashMap, HashSet};

use codegraph_core::extract::{clean_type, is_upper_ident};
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
    /// Rust `crate key|a::b` module → file (see [`crate::rustcrate::module_path`]).
    pub rust_modules: HashMap<String, String>,
    /// Rust crate name (`shop_api`) → crate key (read from `Cargo.toml` by the indexer).
    pub rust_crates: HashMap<String, String>,
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
    /// Part → canonical part of its logical type (precomputed: types can have hundreds).
    part_rep: HashMap<usize, usize>,
    /// Parent id → member name → members (huge partial classes like Unity.Mathematics
    /// `math` have thousands of members; lookups must not scan them).
    pub children_by_name: HashMap<String, HashMap<String, Vec<usize>>>,
    /// C# file → the assembly compiling it (see [`SymbolIndex::build`]).
    pub file_assembly: HashMap<String, String>,
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
    /// `assembly_of` names the assembly compiling a file: C# `partial` parts only merge
    /// within one assembly (copies of a project kept side by side stay separate types).
    pub fn build(
        mut syms: Vec<IndexedSymbol>,
        imports: HashMap<String, Vec<Import>>,
        mut di: Vec<(String, DiRegistration)>,
        assembly_of: &dyn Fn(&str) -> String,
        rust_crates: HashMap<String, String>,
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
            rust_modules: HashMap::new(),
            rust_crates,
            imports,
            file_namespace: HashMap::new(),
            bases: HashMap::new(),
            impls: HashMap::new(),
            di: HashMap::new(),
            parts: HashMap::new(),
            part_rep: HashMap::new(),
            children_by_name: HashMap::new(),
            file_assembly: HashMap::new(),
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
                    Language::Rust if !crate::rustcrate::is_shared_root(&s.file) => {
                        let key = rust_key(&crate::rustcrate::module_path(&s.file));
                        // `src/lib.rs` is the crate root other crates (and `main.rs`) import.
                        let lib = s.file.ends_with("/lib.rs") || s.file == "lib.rs";
                        if lib || !idx.rust_modules.contains_key(&key) {
                            idx.rust_modules.insert(key, s.file.clone());
                        }
                    }
                    _ => {}
                }
                continue;
            }
            if s.sym.kind == SymbolKind::Impl {
                // An `impl` block is reached through the type it implements, never by name.
                if let Some(p) = &s.sym.parent_id {
                    idx.children.entry(p.clone()).or_default().push(i);
                }
                continue;
            }
            idx.by_name.entry(s.sym.name.clone()).or_default().push(i);
            if let Some(p) = &s.sym.parent_id {
                idx.children.entry(p.clone()).or_default().push(i);
                idx.children_by_name.entry(p.clone()).or_default().entry(s.sym.name.clone()).or_default().push(i);
            }
            if let Some(ns) = &s.sym.namespace {
                let list = idx.file_namespace.entry(s.file.clone()).or_default();
                if !list.contains(ns) {
                    list.push(ns.clone());
                }
            }
        }
        for (file, lang) in &idx.file_lang {
            if lang.family() == "dotnet" {
                idx.file_assembly.insert(file.clone(), assembly_of(file));
            }
        }
        // Logical types declared in several parts.
        let mut groups: HashMap<(String, String, SymbolKind), Vec<usize>> = HashMap::new();
        for (i, s) in idx.syms.iter().enumerate() {
            if !s.sym.kind.is_type() {
                continue;
            }
            let key = if s.lang.family() == "dotnet" && s.sym.modifiers.iter().any(|m| m == "partial") {
                let asm = idx.file_assembly.get(&s.file).cloned().unwrap_or_default();
                (format!("dotnet:{asm}"), s.sym.qualified_name.clone(), s.sym.kind)
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
                let rep = g.iter().copied().min().unwrap_or_default();
                for i in &g {
                    idx.parts.insert(*i, g.clone());
                    idx.part_rep.insert(*i, rep);
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
        if !idx.rust_modules.is_empty() {
            idx.link_rust_impls();
        }
        idx.link_bases();
        idx
    }

    /// Inheritance: resolves every declared base (and the reverse implementation map).
    fn link_bases(&mut self) {
        self.bases.clear();
        self.impls.clear();
        let type_ids: Vec<usize> = (0..self.syms.len()).filter(|i| self.syms[*i].sym.kind.is_type()).collect();
        for i in type_ids {
            let s = &self.syms[i];
            let file = s.file.clone();
            let id = s.sym.id.clone();
            let bases = s.sym.bases.clone();
            let mut resolved = Vec::new();
            for b in bases {
                if let Resolved::One(t, c, _) = self.resolve_type(&b, &file) {
                    let tid = self.syms[t].sym.id.clone();
                    if tid != id {
                        resolved.push((tid, c));
                    }
                }
            }
            for (tid, _) in &resolved {
                self.impls.entry(tid.clone()).or_default().push(id.clone());
            }
            self.bases.insert(id, resolved);
        }
    }

    /// Rust `impl` blocks are parts of the type they implement (in any file of the project);
    /// methods of an `impl` for a type outside the project (`impl Ext for String`) are
    /// extension methods of that type.
    fn link_rust_impls(&mut self) {
        let rust_types: Vec<usize> =
            (0..self.syms.len()).filter(|i| self.syms[*i].lang == Language::Rust && self.syms[*i].sym.kind.is_type()).collect();
        for i in &rust_types {
            self.parts.remove(i);
            self.part_rep.remove(i);
        }
        let syms = &self.syms;
        for v in self.extensions.values_mut() {
            v.retain(|(i, _)| syms[*i].lang != Language::Rust);
        }
        let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
        let mut ext: Vec<(String, usize, String)> = Vec::new();
        for i in rust_types.into_iter().filter(|i| self.syms[*i].sym.kind == SymbolKind::Impl) {
            let s = &self.syms[i];
            let target = s.sym.type_ref.clone().unwrap_or_default();
            match self.resolve_type(&target, &s.file) {
                Resolved::One(t, _, _) if self.syms[t].sym.kind != SymbolKind::Impl => groups.entry(t).or_default().push(i),
                Resolved::One(..) | Resolved::Many(_) => {}
                Resolved::None => {
                    let tn = clean_type(&target);
                    let tn = tn.rsplit("::").next().unwrap_or(&tn).to_string();
                    for k in self.children.get(&s.sym.id).cloned().unwrap_or_default() {
                        if self.syms[k].sym.kind.is_callable() {
                            ext.push((self.syms[k].sym.name.clone(), k, tn.clone()));
                        }
                    }
                }
            }
        }
        for (t, impls) in groups {
            let mut g = vec![t];
            g.extend(impls);
            for p in &g {
                self.parts.insert(*p, g.clone());
                self.part_rep.insert(*p, t);
            }
        }
        for (name, k, tn) in ext {
            self.extensions.entry(name).or_default().push((k, tn));
        }
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
        let mut seen_groups = HashSet::new();
        let mut stack = vec![(type_idx, 0)];
        while let Some((t, depth)) = stack.pop() {
            let tid = &self.syms[t].sym.id;
            if !seen.insert(tid.clone()) {
                continue;
            }
            // Partial declarations contribute members (and bases) to the same type; each
            // group is expanded once.
            if let Some(parts) = self.parts.get(&t) {
                if seen_groups.insert(self.representative(t)) {
                    stack.extend(parts.iter().filter(|p| **p != t).map(|p| (*p, depth)));
                }
            }
            if let Some(found) = self.children_by_name.get(tid).and_then(|m| m.get(name)) {
                out.extend(found.iter().copied());
                continue; // nearest declaration wins (override)
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

    /// Of several equally good type candidates, the ones compiled with `from_file` (a C#
    /// compiler binds a name to its own assembly's type first); all of them when none is.
    pub fn prefer_same_assembly(&self, from_file: &str, cands: Vec<usize>) -> Vec<usize> {
        let Some(asm) = self.file_assembly.get(from_file) else { return cands };
        let same: Vec<usize> = cands.iter().copied().filter(|c| self.file_assembly.get(&self.syms[*c].file) == Some(asm)).collect();
        if same.is_empty() {
            cands
        } else {
            same
        }
    }

    /// Every declaration of the logical type `t` (just `t` unless it is partial / merged).
    pub fn parts_of(&self, t: usize) -> Vec<usize> {
        self.parts.get(&t).cloned().unwrap_or_else(|| vec![t])
    }

    /// Canonical declaration standing for the logical type `t`.
    pub fn representative(&self, t: usize) -> usize {
        self.part_rep.get(&t).copied().unwrap_or(t)
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
            Some(Language::Rust) => self.resolve_rust_module(from_file, spec),
            _ => None,
        }
    }

    /// Resolves a Rust module path (`crate::models`, `self::repo`, `super::x`, `shared_kit`,
    /// or a child module named directly) to the file declaring that module.
    pub fn resolve_rust_module(&self, from_file: &str, spec: &str) -> Option<(String, Confidence)> {
        let (ck, cur) = crate::rustcrate::module_path(from_file);
        let segs: Vec<String> = spec.split("::").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let first = segs.first()?.as_str();
        let mut cands: Vec<(String, Vec<String>)> = Vec::new();
        match first {
            "crate" => cands.push((ck.clone(), segs[1..].to_vec())),
            "self" => cands.push((ck.clone(), cur.iter().cloned().chain(segs[1..].iter().cloned()).collect())),
            "super" => {
                let ups = segs.iter().take_while(|s| *s == "super").count();
                let base = cur.len().checked_sub(ups)?;
                cands.push((ck.clone(), cur[..base].iter().cloned().chain(segs[ups..].iter().cloned()).collect()));
            }
            name => {
                // 2018 uniform paths: a child module in scope, then a crate root module, then
                // another crate of the workspace.
                cands.push((ck.clone(), cur.iter().cloned().chain(segs.iter().cloned()).collect()));
                cands.push((ck.clone(), segs.clone()));
                if let Some(other) = self.rust_crates.get(name) {
                    cands.push((other.clone(), segs[1..].to_vec()));
                }
            }
        }
        cands.into_iter().find_map(|c| {
            // A crate root names itself: `crate::` in `main.rs` (next to a `lib.rs`) or in a
            // `tests/*.rs` file is that file, not the library / a sibling test.
            if matches!(first, "crate" | "self") && c.0 == ck && c.1.is_empty() && cur.is_empty() {
                return Some((from_file.to_string(), Confidence::Exact));
            }
            self.rust_modules.get(&rust_key(&c)).map(|f| (f.clone(), Confidence::Exact))
        })
    }

    /// Resolves a Rust path (`Foo`, `models::Order`, `crate::repo::MemoryRepo`,
    /// `Status::Paid`) to a module, item or member, as seen from `from_file`.
    pub fn resolve_rust_path(&self, from_file: &str, path: &str) -> Option<(usize, Confidence, String)> {
        let mut segs: Vec<String> = path.split("::").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let first = segs.first()?.clone();
        // A `use` / `mod` binding of the head: continue from the path it names.
        if !matches!(first.as_str(), "crate" | "self" | "super") {
            if let Some(imp) = self.import_binding(from_file, &first) {
                let mut full: Vec<String> = imp.module.split("::").map(|s| s.to_string()).collect();
                if let Some(i) = &imp.imported {
                    full.push(i.clone());
                }
                full.extend(segs.drain(1..));
                segs = full;
            }
        }
        // Longest module prefix, then items / members.
        for k in (1..=segs.len()).rev() {
            let Some((file, conf)) = self.resolve_rust_module(from_file, &segs[..k].join("::")) else { continue };
            let module = *self.module_of_file.get(&file)?;
            let mut cur = module;
            let mut rest = &segs[k..];
            while let Some(seg) = rest.first() {
                match self.rust_member(cur, seg) {
                    Some(m) => {
                        cur = m;
                        rest = &rest[1..];
                    }
                    // Items of inline modules (`mod outer { pub fn f() }`) live in the file's
                    // module, qualified by the inline module names (`outer.f`).
                    None if cur == module && rest.len() > 1 => {
                        let (q, kind_ok) = (rest.join("."), |s: &IndexedSymbol| s.sym.kind != SymbolKind::Impl);
                        let hit = self.by_file.get(&file)?.iter().copied().find(|i| self.syms[*i].sym.qualified_name == q && kind_ok(&self.syms[*i]))?;
                        return Some((hit, conf, format!("path {path}")));
                    }
                    None => return None,
                }
            }
            return Some((cur, conf, format!("path {path}")));
        }
        // An inline module of this file: `util::shout()` next to `mod util { … }`.
        if segs.len() > 1 {
            let q = segs.join(".");
            if let Some(hit) = self.by_file.get(from_file).and_then(|v| {
                v.iter().copied().find(|i| self.syms[*i].sym.qualified_name == q && self.syms[*i].sym.kind != SymbolKind::Impl)
            }) {
                return Some((hit, Confidence::Exact, format!("inline module path {path}")));
            }
        }
        // A type in scope: `Foo::new`, `Status::Paid`.
        let head = segs.first()?;
        if let Resolved::One(mut cur, conf, ev) = self.resolve_type(head, from_file) {
            for seg in &segs[1..] {
                cur = self.rust_member(cur, seg)?;
            }
            return Some((cur, conf, ev));
        }
        None
    }

    /// Item `name` of a Rust module, or member `name` of a type.
    fn rust_member(&self, owner: usize, name: &str) -> Option<usize> {
        let s = &self.syms[owner];
        if s.sym.kind == SymbolKind::Module {
            return self.exported_symbol(&s.file, name, 0);
        }
        if s.sym.kind.is_type() {
            return self.members_named(owner, name).into_iter().min();
        }
        None
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
            if let Some(k) = kids.iter().find(|k| self.syms[**k].sym.name == name && self.syms[**k].sym.kind != SymbolKind::Impl) {
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
        // Rust paths: `models::Order`, `crate::repo::OrderRepo`.
        if family == "rust" && cleaned.contains("::") {
            if let Some((t, c, ev)) = self.resolve_rust_path(from_file, &cleaned) {
                if self.syms[t].sym.kind.is_type() {
                    return Resolved::One(self.representative(t), c, ev);
                }
            }
            // `reqwest::Client`, `std::fmt::Display`: a path into another (external) crate.
            let head = cleaned.split("::").next().unwrap_or("");
            let project_head = matches!(head, "crate" | "self" | "super")
                || is_upper_ident(head)
                || self.resolve_rust_module(from_file, head).is_some()
                || self.import_binding(from_file, head).map(|i| self.resolve_module(from_file, &i.module).is_some()).unwrap_or(false);
            if !project_head {
                return Resolved::None;
            }
            let last = cleaned.rsplit("::").next().unwrap_or(&cleaned).to_string();
            return self.resolve_type_inner(&last, from_file, ns, aliases);
        }
        // Qualified names: `Shop.Models.Order`, `models.Order`, `ns.Type`.
        if cleaned.contains('.') {
            let last_seg = cleaned.rsplit('.').next().unwrap_or(&cleaned);
            let mut exact: Vec<usize> = self
                .by_name
                .get(last_seg)
                .map(|v| v.iter().copied().filter(|i| self.syms[*i].sym.kind.is_type() && self.syms[*i].sym.qualified_name == cleaned).collect())
                .unwrap_or_default();
            if !self.parts.is_empty() {
                exact = exact.into_iter().map(|c| self.representative(c)).collect();
                exact.sort_unstable();
                exact.dedup();
            }
            if exact.len() > 1 {
                exact = self.prefer_same_assembly(from_file, exact);
            }
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
                let same = self.prefer_same_assembly(from_file, visible.clone());
                if same.len() == 1 {
                    return Resolved::One(same[0], Confidence::Exact, "type match in the same assembly".into());
                }
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
                            SymbolKind::Method
                                | SymbolKind::Constructor
                                | SymbolKind::Property
                                | SymbolKind::Field
                                | SymbolKind::Test
                                | SymbolKind::Impl
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

/// Key of a Rust module in [`SymbolIndex::rust_modules`].
fn rust_key((crate_key, path): &(String, Vec<String>)) -> String {
    format!("{crate_key}|{}", path.join("::"))
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}
