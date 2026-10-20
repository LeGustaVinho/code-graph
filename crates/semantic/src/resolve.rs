//! Per-file resolution: CALLS, INHERITS, IMPORTS, READS_VAR, WRITES_VAR and THROWS edges.

use std::collections::HashSet;

use codegraph_core::extract::clean_type;
use codegraph_core::{ImportKind, Language, RefKind, Reference, SymbolKind};
use serde_json::json;

use crate::index::{Resolved, SymbolIndex};
use crate::{file_name, relation, Confidence, Edge};

/// Method names too generic to resolve without a known receiver type.
const STOP_NAMES: &[&str] = &[
    "map",
    "filter",
    "forEach",
    "reduce",
    "push",
    "pop",
    "shift",
    "then",
    "catch",
    "finally",
    "get",
    "set",
    "add",
    "remove",
    "delete",
    "has",
    "keys",
    "values",
    "entries",
    "toString",
    "valueOf",
    "join",
    "split",
    "slice",
    "splice",
    "concat",
    "includes",
    "indexOf",
    "find",
    "some",
    "every",
    "sort",
    "length",
    "append",
    "extend",
    "update",
    "items",
    "copy",
    "format",
    "strip",
    "replace",
    "lower",
    "upper",
    "startswith",
    "endswith",
    "json",
    "send",
    "emit",
    "on",
    "off",
    "log",
    "error",
    "warn",
    "info",
    "debug",
    "Add",
    "Remove",
    "Contains",
    "ToString",
    "Equals",
    "GetHashCode",
    "Select",
    "Where",
    "First",
    "FirstOrDefault",
    "Any",
    "Count",
    "ToList",
    "Dispose",
    "close",
    "open",
    "read",
    "write",
    "call",
    "apply",
    "bind",
    "next",
    "run",
    "start",
    "stop",
    "init",
    "create",
    "clear",
    "assign",
    "parse",
    "stringify",
    "resolve",
    "reject",
    "all",
    "test",
    "exec",
    "match",
    "trim",
];

const MAX_AMBIGUOUS: usize = 12;

/// Language keyword / primitive types that can appear as static call receivers.
const BUILTIN_TYPES: &[&str] = &[
    "string", "int", "long", "short", "byte", "sbyte", "uint", "ulong", "ushort", "float", "double", "decimal", "bool", "char", "object",
    "nint", "nuint", "dynamic", "Math", "Mathf", "String", "Int32", "Int64", "Boolean", "Double", "Single", "Object", "Array", "Number",
    "JSON", "Promise", "console", "str", "dict", "list", "len",
];

enum Chain {
    Type(usize, Confidence, String),
    External,
    Unknown,
}

/// Outcome of resolving a reference's static type.
enum TypeOf {
    /// A project type.
    Project(usize, Confidence, String),
    /// Known to be outside the project (framework / engine / package type).
    External,
    /// Several project types match, or the type cannot be determined.
    Unknown,
}

pub struct FileResolver<'a> {
    pub index: &'a SymbolIndex,
    pub file: &'a str,
    pub lang: Language,
    family: &'static str,
    edges: Vec<Edge>,
    seen: HashSet<(String, String, String, u32)>,
}

impl<'a> FileResolver<'a> {
    pub fn new(index: &'a SymbolIndex, file: &'a str) -> Self {
        let lang = index.file_lang.get(file).copied().unwrap_or(Language::TypeScript);
        FileResolver { index, file, lang, family: lang.family(), edges: Vec::new(), seen: HashSet::new() }
    }

    fn push(&mut self, e: Edge) {
        let key = (
            e.source_id.clone(),
            e.target_id.clone().or_else(|| e.unresolved_symbol.clone()).unwrap_or_default(),
            e.relation.clone(),
            e.line,
        );
        if self.seen.insert(key) {
            self.edges.push(e);
        }
    }

    fn target(&self, i: usize) -> (&str, &str) {
        let s = &self.index.syms[i];
        (s.sym.id.as_str(), s.file.as_str())
    }

    pub fn resolve(mut self, refs: &[Reference]) -> Vec<Edge> {
        self.inherits();
        self.imports();
        for r in refs {
            match r.kind {
                RefKind::Call => self.call(r),
                RefKind::New | RefKind::Jsx => self.construct(r),
                RefKind::Throw => self.throw(r),
                RefKind::Read | RefKind::Write => self.var_access(r),
                RefKind::TypeUse => self.type_use(r),
                RefKind::Catch => {}
            }
        }
        self.edges
    }

    // -----------------------------------------------------------------------

    fn inherits(&mut self) {
        let Some(ids) = self.index.by_file.get(self.file) else { return };
        let types: Vec<usize> = ids.iter().copied().filter(|i| self.index.syms[*i].sym.kind.is_type()).collect();
        for t in types {
            let sym = self.index.syms[t].sym.clone();
            for raw in &sym.bases {
                let base = clean_type(raw);
                if base.is_empty() || base == "object" {
                    continue;
                }
                match self.index.resolve_type_ns(raw, self.file, sym.namespace.as_deref()) {
                    Resolved::One(b, conf, ev) => {
                        let bk = self.index.syms[b].sym.kind;
                        let kind = if bk == SymbolKind::Interface { "implements" } else { "extends" };
                        let e = Edge::new(
                            &sym.id,
                            self.file,
                            Some(self.target(b)),
                            relation::INHERITS,
                            conf,
                            format!("{kind} {base}: {ev}"),
                            "inheritance",
                            sym.start_line,
                        )
                        .with_meta(json!({ "kind": kind }));
                        self.push(e);
                    }
                    Resolved::Many(c) => {
                        let e = Edge::unresolved(
                            &sym.id,
                            self.file,
                            relation::INHERITS,
                            &base,
                            format!("{} candidate types named {base}", c.len()),
                            "inheritance",
                            sym.start_line,
                        );
                        self.push(e);
                    }
                    Resolved::None => {
                        let kind = if self.lang == Language::CSharp && looks_like_interface(&base) { "implements" } else { "extends" };
                        let e = Edge::unresolved(
                            &sym.id,
                            self.file,
                            relation::INHERITS,
                            &base,
                            format!("external base type '{base}' (not defined in project)"),
                            "inheritance",
                            sym.start_line,
                        )
                        .with_meta(json!({ "kind": kind, "external": true }));
                        self.push(e);
                    }
                }
            }
        }
    }

    fn imports(&mut self) {
        let Some(imps) = self.index.imports.get(self.file) else { return };
        let Some(&m) = self.index.module_of_file.get(self.file) else { return };
        let module_id = self.index.syms[m].sym.id.clone();
        for imp in imps.clone() {
            if matches!(imp.kind, ImportKind::Using | ImportKind::UsingStatic) {
                continue;
            }
            match self.index.resolve_module(self.file, &imp.module) {
                Some((target_file, conf)) => {
                    let named = match imp.kind {
                        ImportKind::Named | ImportKind::Default => {
                            imp.imported.as_deref().and_then(|n| self.index.exported_symbol(&target_file, n, 0))
                        }
                        _ => None,
                    };
                    let tgt = named.or_else(|| self.index.module_of_file.get(&target_file).copied());
                    if let Some(t) = tgt {
                        let what = match (&imp.imported, imp.kind) {
                            (Some(n), ImportKind::Named) => format!("import {{ {n} }} from '{}'", imp.module),
                            (_, ImportKind::Default) => format!("default import from '{}'", imp.module),
                            _ => format!("import '{}'", imp.module),
                        };
                        let e = Edge::new(
                            &module_id,
                            self.file,
                            Some(self.target(t)),
                            relation::IMPORTS,
                            conf,
                            what,
                            "import-linker",
                            imp.line,
                        )
                        .with_meta(json!({ "module": imp.module, "local": imp.local }));
                        self.push(e);
                    }
                }
                None => {
                    if imp.module.starts_with('.') {
                        let e = Edge::unresolved(
                            &module_id,
                            self.file,
                            relation::IMPORTS,
                            &imp.module,
                            format!("relative module '{}' not found in index", imp.module),
                            "import-linker",
                            imp.line,
                        );
                        self.push(e);
                    }
                }
            }
        }
    }

    fn enclosing_type(&self, enclosing: &str) -> Option<usize> {
        self.index.enclosing_type(enclosing)
    }

    fn callables(&self, v: Vec<usize>) -> Vec<usize> {
        v.into_iter().filter(|i| self.index.syms[*i].sym.kind.is_callable() || self.index.syms[*i].sym.kind.is_type()).collect()
    }

    fn call_meta(r: &Reference) -> serde_json::Value {
        let mut m = json!({});
        if let Some(a) = r.arg_count {
            m["args"] = json!(a);
        }
        if !r.catch_types.is_empty() {
            m["try"] = json!(r.catch_types);
        }
        if let Some(rc) = &r.receiver {
            m["receiver"] = json!(rc);
        }
        m
    }

    fn emit_call(&mut self, r: &Reference, t: usize, conf: Confidence, evidence: String, resolver: &str) {
        // Calling a type means constructing it: target its constructor when declared.
        let mut target = t;
        let mut evidence = evidence;
        if self.index.syms[t].sym.kind.is_type() {
            // Static constructors (type initialisers) are never the target of `new`; partial
            // types may declare constructors in any part.
            let ctors: Vec<usize> = self
                .index
                .parts_of(t)
                .into_iter()
                .flat_map(|p| self.index.children.get(&self.index.syms[p].sym.id).cloned().unwrap_or_default())
                .filter(|k| self.index.syms[*k].sym.kind == SymbolKind::Constructor && !self.index.syms[*k].sym.is_static)
                .collect();
            let ctor = ctors
                .iter()
                .copied()
                .find(|c| r.arg_count.map(|a| accepts_args(&self.index.syms[*c].sym.params, a)).unwrap_or(true))
                .or_else(|| ctors.first().copied());
            if let Some(c) = ctor {
                target = c;
                evidence = format!("{evidence} (constructor)");
            }
        }
        let e = Edge::new(&r.enclosing, self.file, Some(self.target(target)), relation::CALLS, conf, evidence, resolver, r.line)
            .with_meta(Self::call_meta(r));
        self.push(e);
    }

    fn emit_unresolved_call(&mut self, r: &Reference, symbol: &str, evidence: String, candidates: &[usize]) {
        let mut meta = Self::call_meta(r);
        // Canonical order before truncating, so the kept candidates never depend on load order.
        let mut candidates = candidates.to_vec();
        self.index.sort_canonical(&mut candidates);
        candidates.dedup();
        let cands: Vec<String> =
            candidates.iter().take(8).map(|c| format!("{} {}", self.index.syms[*c].sym.qualified_name, self.index.coords(*c))).collect();
        meta["candidates"] = json!(cands);
        meta["candidate_ids"] = json!(candidates.iter().take(8).map(|c| self.index.syms[*c].sym.id.clone()).collect::<Vec<_>>());
        let e = Edge::unresolved(&r.enclosing, self.file, relation::CALLS, symbol, evidence, "call-resolver", r.line).with_meta(meta);
        self.push(e);
    }

    /// Dispatches a call on an abstract type to its concrete implementation(s).
    fn dispatch(&mut self, r: &Reference, type_idx: usize, method: &str, field: Option<&str>) {
        let tname = self.index.syms[type_idx].sym.name.clone();
        let tid = self.index.syms[type_idx].sym.id.clone();
        // 1. DI container registrations.
        let mut regs = self.index.di.get(&(self.family.to_string(), tname.clone())).cloned().unwrap_or_default();
        // NestJS @Inject('TOKEN') constructor parameters.
        if let (Some(f), Some(cls)) = (field, self.enclosing_type(&r.enclosing)) {
            if let Some(token) = inject_token(self.index, cls, f) {
                if let Some(t) = self.index.di.get(&(self.family.to_string(), token)) {
                    regs.extend(t.iter().cloned());
                }
            }
        }
        let mut done = false;
        for reg in &regs {
            let Some(imp) = &reg.reg.implementation else { continue };
            if let Resolved::One(ci, _, _) = self.index.resolve_type(imp, &reg.file) {
                for m in self.index.members_named(ci, method) {
                    if self.index.syms[m].sym.kind.is_callable() && self.index.syms[m].sym.id != self.index.syms[type_idx].sym.id {
                        let ev = format!(
                            "DI {} registration {} -> {} in {}:{}",
                            reg.reg.lifetime,
                            reg.reg.service,
                            imp,
                            file_name(&reg.file),
                            reg.reg.line
                        );
                        self.emit_call(r, m, Confidence::Exact, ev, "di-resolver");
                        done = true;
                    }
                }
            }
        }
        if done {
            return;
        }
        // 2. Implementations in the project.
        let impls: Vec<usize> = self.index.implementations(&tid).into_iter().filter(|i| !self.index.is_abstract_type(*i)).collect();
        let targets: Vec<usize> = impls
            .iter()
            .flat_map(|i| {
                self.index
                    .children
                    .get(&self.index.syms[*i].sym.id)
                    .map(|k| k.iter().copied().filter(|c| self.index.syms[*c].sym.name == method).collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect();
        match targets.len() {
            0 => {}
            1 => {
                let impl_name = self
                    .index
                    .get(self.index.syms[targets[0]].sym.parent_id.as_deref().unwrap_or(""))
                    .map(|p| p.sym.name.clone())
                    .unwrap_or_default();
                self.emit_call(
                    r,
                    targets[0],
                    Confidence::Inferred,
                    format!("single implementation {impl_name} of {tname}"),
                    "dispatch-resolver",
                );
            }
            n => {
                let sym = format!("{tname}.{method}");
                // Production code never dispatches to test doubles: if exactly one candidate is
                // a real implementation, link it (inferred) and name the excluded doubles.
                let caller_is_test =
                    self.index.get(&r.enclosing).map(|s| s.sym.is_test || codegraph_core::extract::is_test_path(&s.file)).unwrap_or(false);
                if !caller_is_test {
                    let (doubles, real): (Vec<usize>, Vec<usize>) = targets.iter().partition(|t| self.is_test_double(**t));
                    if real.len() == 1 && !doubles.is_empty() {
                        let owner = |i: usize| {
                            self.index
                                .get(self.index.syms[i].sym.parent_id.as_deref().unwrap_or(""))
                                .map(|p| p.sym.name.clone())
                                .unwrap_or_default()
                        };
                        let excluded: Vec<String> = doubles.iter().map(|d| owner(*d)).collect();
                        let ev = format!(
                            "single production implementation {} of {tname} (test doubles excluded: {})",
                            owner(real[0]),
                            excluded.join(", ")
                        );
                        self.emit_call(r, real[0], Confidence::Inferred, ev, "dispatch-resolver");
                        return;
                    }
                }
                self.emit_unresolved_call(r, &sym, format!("{n} potential implementations of {sym}"), &targets);
            }
        }
    }

    fn call_on_type(&mut self, r: &Reference, t: usize, conf: Confidence, ev: String, field: Option<&str>) -> bool {
        let mut methods: Vec<usize> = self
            .index
            .members_named(t, &r.name)
            .into_iter()
            .filter(|m| {
                let s = &self.index.syms[*m].sym;
                s.kind.is_callable() && !(s.kind == SymbolKind::Constructor && s.is_static)
            })
            .collect();
        if methods.is_empty() {
            return false;
        }
        // Overload resolution by arity: keep only the overloads that accept this many arguments.
        if methods.len() > 1 {
            if let Some(argc) = r.arg_count {
                let fit: Vec<usize> = methods.iter().copied().filter(|m| accepts_args(&self.index.syms[*m].sym.params, argc)).collect();
                if !fit.is_empty() {
                    methods = fit;
                }
            }
        }
        for m in &methods {
            self.emit_call(r, *m, conf, ev.clone(), "call-resolver");
        }
        // Virtual dispatch on interfaces / abstract members.
        let decl_owner = self.index.syms[methods[0]].sym.parent_id.clone().and_then(|p| self.index.by_id.get(&p).copied());
        let abstract_target = self.index.is_abstract_type(t) || methods.iter().any(|m| self.index.syms[*m].sym.is_abstract);
        if abstract_target {
            let owner = decl_owner.unwrap_or(t);
            self.dispatch(r, owner, &r.name, field);
        } else if methods.iter().any(|m| self.index.syms[*m].sym.modifiers.iter().any(|x| x == "virtual" || x == "override")) {
            self.dispatch_virtual(r, &methods);
        }
        true
    }

    /// A call to a virtual member may run any override: link them (inferred), never ambiguity.
    fn dispatch_virtual(&mut self, r: &Reference, methods: &[usize]) {
        let mut targets = Vec::new();
        for m in methods {
            let Some(owner) = self.index.syms[*m].sym.parent_id.clone() else { continue };
            for sub in self.index.implementations(&owner) {
                let sub_id = self.index.syms[sub].sym.id.clone();
                for k in self.index.children.get(&sub_id).cloned().unwrap_or_default() {
                    let s = &self.index.syms[k].sym;
                    if s.name == r.name && s.modifiers.iter().any(|x| x == "override") && !targets.contains(&k) {
                        targets.push(k);
                    }
                }
            }
        }
        targets.sort_unstable();
        for k in targets.into_iter().take(8) {
            let owner =
                self.index.get(self.index.syms[k].sym.parent_id.as_deref().unwrap_or("")).map(|p| p.sym.name.clone()).unwrap_or_default();
            self.emit_call(r, k, Confidence::Inferred, format!("virtual dispatch to override in {owner}"), "dispatch-resolver");
        }
    }

    /// `items.ToDto()` → `static Dto ToDto(this Item x)`: an extension method on the receiver
    /// type (or one of its ancestors), or a generic extension (`this T x`).
    fn extension_call(&mut self, r: &Reference, owner: Option<usize>, raw: &str) -> bool {
        let Some(cands) = self.index.extensions.get(&r.name).cloned() else { return false };
        let names: HashSet<String> = match owner {
            Some(t) => self.type_names(t),
            None => [clean_type(raw)].into_iter().collect(),
        };
        let is_generic = |ext: &str| ext.len() <= 2 && ext.starts_with('T') || ext == "object";
        let exact: Vec<usize> = cands.iter().filter(|(_, ext)| names.contains(ext)).map(|(i, _)| *i).collect();
        if !exact.is_empty() {
            let conf = if exact.len() == 1 { Confidence::Exact } else { Confidence::Inferred };
            let tn = names.iter().min().cloned().unwrap_or_default();
            for i in exact.into_iter().take(4) {
                self.emit_call(r, i, conf, format!("extension method {}(this {tn})", r.name), "extension-resolver");
            }
            return true;
        }
        let generic: Vec<usize> = cands.iter().filter(|(_, ext)| is_generic(ext)).map(|(i, _)| *i).collect();
        if generic.len() == 1 {
            self.emit_call(r, generic[0], Confidence::Inferred, format!("generic extension method {}", r.name), "extension-resolver");
            return true;
        }
        false
    }

    /// Names of a project type, its other partial declarations and every ancestor (project
    /// types and declared external bases).
    fn type_names(&self, t: usize) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut stack = vec![t];
        let mut seen = HashSet::new();
        while let Some(x) = stack.pop() {
            if !seen.insert(x) || seen.len() > 64 {
                continue;
            }
            for p in self.index.parts_of(x) {
                let s = &self.index.syms[p].sym;
                out.insert(s.name.clone());
                for b in &s.bases {
                    out.insert(clean_type(b));
                }
                for (b, _) in self.index.bases.get(&s.id).cloned().unwrap_or_default() {
                    if let Some(bi) = self.index.by_id.get(&b) {
                        stack.push(*bi);
                    }
                }
            }
        }
        out
    }

    /// Static type of a receiver: plain names, or deferred member types (`@mt#Owner#member`)
    /// recorded for `var x = repo.Find(id)` / `GetUser().Profile` / `(await svc.Load()).Name`.
    fn ref_type(&self, raw: &str, r: &Reference, depth: u32) -> TypeOf {
        let Some(rest) = raw.strip_prefix("@mt#") else {
            return match self.ty(raw, r) {
                Resolved::One(t, c, ev) => TypeOf::Project(t, c, ev),
                Resolved::None => TypeOf::External,
                Resolved::Many(_) => TypeOf::Unknown,
            };
        };
        if depth > 6 {
            return TypeOf::Unknown;
        }
        let Some((owner_raw, member)) = rest.rsplit_once('#') else { return TypeOf::Unknown };
        let (member, awaited) = match member.strip_suffix("!await") {
            Some(m) => (m, true),
            None => (member, false),
        };
        // `@fn`: the declared return type of a free function visible from here.
        if owner_raw == "@fn" {
            let f = match self.index.resolve_value(member, self.file, &r.enclosing) {
                Resolved::One(f, _, _) => f,
                _ => return TypeOf::Unknown,
            };
            let fs = &self.index.syms[f];
            let mut ty = fs.sym.type_ref.clone().unwrap_or_default();
            if awaited && matches!(clean_type(&ty).as_str(), "Promise" | "Task") {
                ty = codegraph_core::extract::generic_args(&ty).into_iter().next().unwrap_or_default();
            }
            if ty.is_empty() {
                return TypeOf::Unknown;
            }
            let file = fs.file.clone();
            return match self.index.resolve_type(&ty, &file) {
                Resolved::One(t, c, _) => TypeOf::Project(t, c, format!("return type of {member}()")),
                Resolved::None => TypeOf::External,
                Resolved::Many(_) => TypeOf::Unknown,
            };
        }
        let owner = if owner_raw.is_empty() {
            match self.enclosing_type(&r.enclosing) {
                Some(t) => t,
                None => return TypeOf::Unknown,
            }
        } else {
            match self.ref_type(owner_raw, r, depth + 1) {
                TypeOf::Project(t, _, _) => t,
                other => return other,
            }
        };
        let found = self.index.members_named(owner, member).into_iter().find(|m| self.index.syms[*m].sym.type_ref.is_some());
        let Some(m) = found else {
            // Not declared in the project: inherited from an external base type.
            return if self.has_external_base(owner) { TypeOf::External } else { TypeOf::Unknown };
        };
        let ms = &self.index.syms[m];
        let mut ty = ms.sym.type_ref.clone().unwrap_or_default();
        if awaited {
            let base = clean_type(&ty);
            if matches!(base.as_str(), "Task" | "ValueTask" | "Promise" | "UniTask" | "Awaitable") {
                ty = codegraph_core::extract::generic_args(&ty).into_iter().next().unwrap_or_default();
            }
        }
        if ty.is_empty() || matches!(clean_type(&ty).as_str(), "void" | "Task" | "ValueTask" | "Promise" | "UniTask" | "IEnumerator") {
            return TypeOf::External;
        }
        let file = ms.file.clone();
        match self.index.resolve_type_ns(&ty, &file, ms.sym.namespace.as_deref()) {
            Resolved::One(t, c, _) => {
                let tn = self.index.syms[t].sym.name.clone();
                TypeOf::Project(t, c.weakest(Confidence::Exact), format!("declared type {tn} of {member}"))
            }
            Resolved::None => TypeOf::External,
            Resolved::Many(_) => TypeOf::Unknown,
        }
    }

    /// `USES_TYPE`: `typeof(T)`, attributes, generic arguments of factory calls.
    fn type_use(&mut self, r: &Reference) {
        let attr = r.detail.as_deref() == Some("attribute");
        let names = if attr { vec![format!("{}Attribute", r.name), r.name.clone()] } else { vec![r.name.clone()] };
        let own = self.enclosing_type(&r.enclosing);
        for name in names {
            if let Resolved::One(t, c, ev) = self.ty(&name, r) {
                if Some(t) == own {
                    return;
                }
                let what = match r.detail.as_deref() {
                    Some("attribute") => format!("attribute [{}]", r.name),
                    Some("typeof") => format!("typeof({})", r.name),
                    Some(d) => d.to_string(),
                    None => format!("type {}", r.name),
                };
                let e = Edge::new(
                    &r.enclosing,
                    self.file,
                    Some(self.target(t)),
                    relation::USES_TYPE,
                    c,
                    format!("{what} ({ev})"),
                    "type-use",
                    r.line,
                );
                self.push(e);
                return;
            }
        }
    }

    /// Resolves a type name as seen from the reference's own namespace.
    fn ty(&self, raw: &str, r: &Reference) -> Resolved {
        let ns = self.index.get(&r.enclosing).and_then(|s| s.sym.namespace.clone());
        self.index.resolve_type_ns(raw, self.file, ns.as_deref())
    }

    /// A function declared inside an enclosing function body (C# local function, nested JS function).
    fn local_function(&self, r: &Reference) -> Option<usize> {
        let mut scope = Some(r.enclosing.clone());
        let mut guard = 0;
        while let Some(sid) = scope {
            let s = self.index.get(&sid)?;
            if s.sym.kind.is_type() || s.sym.kind == SymbolKind::Module {
                return None;
            }
            if let Some(kids) = self.index.children.get(&sid) {
                if let Some(k) =
                    kids.iter().find(|k| self.index.syms[**k].sym.name == r.name && self.index.syms[**k].sym.kind == SymbolKind::Function)
                {
                    return Some(*k);
                }
            }
            scope = s.sym.parent_id.clone();
            guard += 1;
            if guard > 16 {
                break;
            }
        }
        None
    }

    fn call(&mut self, r: &Reference) {
        let file = self.file;
        match r.receiver.as_deref() {
            None => {
                // A local function shadows every other declaration.
                if let Some(f) = self.local_function(r) {
                    self.emit_call(r, f, Confidence::Exact, "local function".into(), "call-resolver");
                    return;
                }
                // C# implicit `this`: methods of the enclosing type.
                if self.lang == Language::CSharp {
                    if let Some(t) = self.enclosing_type(&r.enclosing) {
                        let tn = self.index.syms[t].sym.name.clone();
                        if self.call_on_type(r, t, Confidence::Exact, format!("same-class method of {tn}"), None) {
                            return;
                        }
                        // `using static Shop.Util;` brings static members into scope.
                        for imp in self.index.imports.get(file).cloned().unwrap_or_default() {
                            if imp.kind == ImportKind::UsingStatic {
                                if let Resolved::One(st, c, _) = self.ty(&imp.module, r) {
                                    if self.call_on_type(r, st, c, format!("using static {}", imp.module), None) {
                                        return;
                                    }
                                }
                            }
                        }
                        // Not declared in the project: inherited from an engine/framework base
                        // (`StartCoroutine`, `Instantiate`, `GetComponent`).
                        if self.has_external_base(t) && matches!(self.index.resolve_value(&r.name, file, &r.enclosing), Resolved::None) {
                            return;
                        }
                    }
                }
                match self.index.resolve_value(&r.name, file, &r.enclosing) {
                    Resolved::One(t, c, ev)
                        if self.index.syms[t].sym.kind.is_callable()
                            || self.index.syms[t].sym.kind.is_type()
                            || self.index.syms[t].sym.kind == SymbolKind::Variable
                            || self.index.syms[t].sym.kind == SymbolKind::Const =>
                    {
                        self.emit_call(r, t, c, ev, "call-resolver");
                        return;
                    }
                    _ => {}
                }
                // Class instantiation without `new` (Python) or global functions.
                if let Resolved::One(t, c, ev) = self.ty(&r.name, r) {
                    self.emit_call(r, t, c, ev, "call-resolver");
                    return;
                }
                self.global_by_name(r, false);
            }
            Some(recv) => {
                let recv_l = recv;
                if recv_l == "this" || recv_l == "self" || recv_l == "cls" {
                    if let Some(t) = self.enclosing_type(&r.enclosing) {
                        let tn = self.index.syms[t].sym.name.clone();
                        self.call_on_type(r, t, Confidence::Exact, format!("self reference ({recv_l}.) in {tn}"), None);
                    }
                    return;
                }
                if recv_l == "super" || recv_l == "base" || recv_l.starts_with("super(") {
                    if let Some(t) = self.enclosing_type(&r.enclosing) {
                        let tid = self.index.syms[t].sym.id.clone();
                        for (b, c) in self.index.bases.get(&tid).cloned().unwrap_or_default() {
                            if let Some(bi) = self.index.by_id.get(&b).copied() {
                                if self.call_on_type(r, bi, c, "base class call".into(), None) {
                                    break;
                                }
                            }
                        }
                    }
                    return;
                }
                let head = recv_l.split(['.', '?', '[', '(']).next().unwrap_or(recv_l);
                // Built-in C# / TS type keywords used as static receivers (`string.IsNullOrEmpty`).
                if BUILTIN_TYPES.contains(&head) {
                    return;
                }
                // Members inherited from an external base (`gameObject.SetActive`, `transform.position`).
                if r.receiver_type.is_none()
                    && r.root_type.is_none()
                    && head.chars().next().map(|c| c.is_ascii_lowercase()).unwrap_or(false)
                {
                    if let Some(t) = self.enclosing_type(&r.enclosing) {
                        if self.index.members_named(t, head).is_empty()
                            && self.has_external_base(t)
                            && matches!(self.index.resolve_value(head, file, &r.enclosing), Resolved::None)
                            && self.index.import_binding(file, head).is_none()
                        {
                            return;
                        }
                    }
                }
                let field = recv_l.strip_prefix("this.").or_else(|| recv_l.strip_prefix("self."));
                // Typed receiver (declared / inferred static type).
                if let Some(rt) = &r.receiver_type {
                    match self.ref_type(rt, r, 0) {
                        TypeOf::Project(t, c, ev) => {
                            let tn = self.index.syms[t].sym.name.clone();
                            let how = format!("typed receiver '{recv_l}: {tn}' ({ev})");
                            if self.call_on_type(r, t, c, how, field.or(Some(recv_l))) {
                                return;
                            }
                            self.extension_call(r, Some(t), rt);
                            return; // method inherited from an external type
                        }
                        TypeOf::Unknown => {}
                        TypeOf::External => {
                            // External type (HttpClient, ILogger, Transform, ...): only project
                            // extension methods on it can be targets.
                            if !rt.starts_with("@mt#") {
                                self.extension_call(r, None, rt);
                            }
                            return;
                        }
                    }
                }
                let head = recv_l.split('.').next().unwrap_or(recv_l);
                // Namespace / module import: `utils.hash()`, `models.Item(...)`. C# usings are
                // namespaces / aliases, resolved as types below.
                if !recv_l.contains('(') && self.lang != Language::CSharp {
                    if let Some(imp) = self.index.import_binding(file, head).cloned() {
                        match self.index.resolve_imported(file, head) {
                            Some((m, c, ev)) => {
                                let sym = &self.index.syms[m].sym;
                                if sym.kind == SymbolKind::Module {
                                    let mfile = self.index.syms[m].file.clone();
                                    if let Some(s) = self.index.exported_symbol(&mfile, &r.name, 0) {
                                        self.emit_call(r, s, c, format!("member of imported module '{}'", imp.module), "call-resolver");
                                    }
                                } else if sym.kind.is_type() {
                                    self.call_on_type(r, m, c, format!("static member via {ev}"), None);
                                } else if matches!(sym.kind, SymbolKind::Variable | SymbolKind::Const) {
                                    // `import { api } from './api'; api.list()`: a method of the object.
                                    self.call_on_type(r, m, c, format!("member of imported object '{head}' ({ev})"), None);
                                }
                            }
                            None => {} // external package: not part of the graph
                        }
                        return;
                    }
                }
                // A module-level object literal: `const api = { list() {} }; api.list()`.
                if !recv_l.contains('.') && !recv_l.contains('(') {
                    if let Resolved::One(v, c, _) = self.index.resolve_value(recv_l, file, &r.enclosing) {
                        let vs = &self.index.syms[v].sym;
                        if matches!(vs.kind, SymbolKind::Variable | SymbolKind::Const) && self.index.children.contains_key(&vs.id) {
                            let ev = format!("method of object '{recv_l}'");
                            if self.call_on_type(r, v, c, ev, None) {
                                return;
                            }
                        }
                    }
                }
                // Namespace-qualified calls: `Util.Inner.f()`, `Shop.Util.Clamp()`.
                if recv_l.contains('.') && !recv_l.contains('(') {
                    let q = format!("{recv_l}.{}", r.name);
                    let suffix = format!(".{q}");
                    let family = self.family;
                    let hits: Vec<usize> = self
                        .index
                        .by_name
                        .get(&r.name)
                        .map(|v| {
                            v.iter()
                                .copied()
                                .filter(|i| {
                                    let s = &self.index.syms[*i];
                                    s.lang.family() == family
                                        && s.sym.kind.is_callable()
                                        && (s.sym.qualified_name == q || s.sym.qualified_name.ends_with(&suffix))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if hits.len() == 1 {
                        self.emit_call(r, hits[0], Confidence::Exact, format!("qualified name {q}"), "call-resolver");
                        return;
                    }
                }
                // Static call on a type: `OrderService.Create()`, `Item.query()`.
                if !recv_l.contains('.') && !recv_l.contains('(') && recv_l.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
                {
                    match self.ty(recv_l, r) {
                        Resolved::One(t, c, ev) => {
                            self.call_on_type(r, t, c, format!("static type reference {recv_l} ({ev})"), None);
                            return;
                        }
                        Resolved::None => return,
                        Resolved::Many(_) => {}
                    }
                }
                // Member chains: `GameInstance.Network.Call()`, `_renderer.material.SetColor()`.
                if recv_l.contains('.') && !recv_l.contains('(') && !recv_l.contains('[') {
                    match self.resolve_chain(r, recv_l) {
                        Chain::Type(t, c, ev) => {
                            self.call_on_type(r, t, c, ev, None);
                            return;
                        }
                        Chain::External => return,
                        Chain::Unknown => {}
                    }
                }
                self.global_by_name(r, true);
            }
        }
    }

    /// A method used as a value (`event += OnClick`, `AddListener(OnClick)`, `setTimeout(this.tick)`)
    /// is a reference to that method: record it as a CALLS edge flagged as a delegate.
    fn method_group(&mut self, r: &Reference) -> bool {
        let target = match r.receiver.as_deref() {
            None if self.lang == Language::CSharp => self.enclosing_type(&r.enclosing).and_then(|t| {
                self.index.members_named(t, &r.name).into_iter().find(|m| matches!(self.index.syms[*m].sym.kind, SymbolKind::Method))
            }),
            None => match self.index.resolve_value(&r.name, self.file, &r.enclosing) {
                Resolved::One(t, _, _) if self.index.syms[t].sym.kind == SymbolKind::Function => Some(t),
                _ => None,
            },
            Some("this") | Some("self") => self.enclosing_type(&r.enclosing).and_then(|t| {
                self.index.members_named(t, &r.name).into_iter().find(|m| matches!(self.index.syms[*m].sym.kind, SymbolKind::Method))
            }),
            _ => None,
        };
        let Some(m) = target else { return false };
        let e = Edge::new(
            &r.enclosing,
            self.file,
            Some(self.target(m)),
            relation::CALLS,
            Confidence::Exact,
            format!("method reference {} passed as delegate/callback", r.name),
            "call-resolver",
            r.line,
        )
        .with_meta(json!({ "delegate": true }));
        self.push(e);
        true
    }

    /// Mock / fake / stub implementations, or anything living in a test folder.
    fn is_test_double(&self, member: usize) -> bool {
        let s = &self.index.syms[member];
        let owner = s.sym.parent_id.as_deref().and_then(|p| self.index.get(p)).map(|p| p.sym.name.as_str()).unwrap_or("");
        let lower = owner.to_ascii_lowercase();
        codegraph_core::extract::is_test_path(&s.file)
            || ["mock", "fake", "stub", "dummy", "spy"].iter().any(|p| lower.starts_with(p) || lower.ends_with(p))
    }

    /// True when the type (or one of its project ancestors) derives from a type outside the project.
    fn has_external_base(&self, t: usize) -> bool {
        let mut stack = vec![t];
        let mut seen = HashSet::new();
        while let Some(x) = stack.pop() {
            let s = &self.index.syms[x];
            if !seen.insert(s.sym.id.clone()) {
                continue;
            }
            for p in self.index.parts_of(x) {
                if p != x {
                    stack.push(p);
                }
            }
            let resolved = self.index.bases.get(&s.sym.id).cloned().unwrap_or_default();
            if s.sym.bases.len() > resolved.len() {
                return true;
            }
            stack.extend(resolved.iter().filter_map(|(b, _)| self.index.by_id.get(b).copied()));
        }
        false
    }

    /// Walks `head.member.member` through declared field / property types.
    fn resolve_chain(&self, r: &Reference, recv: &str) -> Chain {
        let segs: Vec<&str> = recv.split('.').map(|s| s.trim_end_matches(['?', '!']).trim()).filter(|s| !s.is_empty()).collect();
        if segs.len() < 2 {
            return Chain::Unknown;
        }
        let upper = |s: &str| s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
        let (mut cur, mut conf, start) = if segs[0] == "this" || segs[0] == "self" || segs[0] == "base" {
            match self.enclosing_type(&r.enclosing) {
                Some(t) => (t, Confidence::Exact, 1),
                None => return Chain::Unknown,
            }
        } else if let Some(rt) = &r.root_type {
            match self.ref_type(rt, r, 0) {
                TypeOf::Project(t, c, _) => (t, c, 1),
                TypeOf::External => return Chain::External,
                TypeOf::Unknown => return Chain::Unknown,
            }
        } else if upper(segs[0]) {
            // Longest namespace-qualified prefix naming a project type (`Game.Core.Foo.Bar`).
            let mut found = None;
            for k in (1..=segs.len()).rev() {
                if k == 1 {
                    if let Resolved::One(t, c, _) = self.ty(segs[0], r) {
                        found = Some((t, c, 1));
                    }
                } else {
                    let prefix = segs[..k].join(".");
                    let suffix = format!(".{prefix}");
                    let hits: Vec<usize> = (0..self.index.syms.len())
                        .filter(|i| {
                            let s = &self.index.syms[*i].sym;
                            s.kind.is_type() && (s.qualified_name == prefix || s.qualified_name.ends_with(&suffix))
                        })
                        .collect();
                    // An exact qualified name beats suffix matches; a unique hit is exact, otherwise
                    // take the canonical first candidate and only claim an inferred link.
                    let exact: Vec<usize> = hits.iter().copied().filter(|i| self.index.syms[*i].sym.qualified_name == prefix).collect();
                    let pool = if exact.is_empty() { hits } else { exact };
                    if let Some(t) = self.index.canonical_first(&pool) {
                        let conf = if pool.len() == 1 { Confidence::Exact } else { Confidence::Inferred };
                        found = Some((t, conf, k));
                        break;
                    }
                }
            }
            match found {
                Some(f) => f,
                // `UnityEngine.Debug`, `System.IO.File`: a qualified external type.
                None if segs.iter().all(|s| upper(s)) => return Chain::External,
                None => return Chain::Unknown,
            }
        } else {
            return Chain::Unknown;
        };
        for seg in &segs[start..] {
            let member = self.index.members_named(cur, seg).into_iter().find(|m| {
                matches!(self.index.syms[*m].sym.kind, SymbolKind::Field | SymbolKind::Property | SymbolKind::Const | SymbolKind::Variable)
            });
            match member {
                Some(m) => {
                    let ms = &self.index.syms[m];
                    let Some(ty) = ms.sym.type_ref.clone() else { return Chain::Unknown };
                    match self.index.resolve_type_ns(&ty, &ms.file, ms.sym.namespace.as_deref()) {
                        Resolved::One(t, c, _) => {
                            cur = t;
                            conf = conf.weakest(c);
                        }
                        Resolved::None => return Chain::External,
                        Resolved::Many(_) => return Chain::Unknown,
                    }
                }
                None if self.has_external_base(cur) => return Chain::External,
                None => return Chain::Unknown,
            }
        }
        let tn = self.index.syms[cur].sym.name.clone();
        Chain::Type(cur, conf, format!("member chain {recv}: {tn}"))
    }

    /// Last resort: match by name across the project (inferred / unresolved).
    fn global_by_name(&mut self, r: &Reference, method_only: bool) {
        if STOP_NAMES.contains(&r.name.as_str()) || r.name.len() < 3 {
            return;
        }
        let family = self.family;
        let cands: Vec<usize> = self
            .index
            .by_name
            .get(&r.name)
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|i| {
                        let s = &self.index.syms[*i];
                        s.lang.family() == family
                            && if method_only {
                                matches!(s.sym.kind, SymbolKind::Method)
                            } else {
                                matches!(s.sym.kind, SymbolKind::Function)
                                    || (self.lang == Language::CSharp && s.sym.kind == SymbolKind::Method && s.sym.is_static)
                            }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cands = self.callables(cands);
        match cands.len() {
            0 => {}
            1 => {
                let what = if method_only {
                    format!("unique method name '.{}()' in project", r.name)
                } else {
                    format!("unique function name '{}' in project", r.name)
                };
                self.emit_call(r, cands[0], Confidence::Inferred, what, "name-matcher");
            }
            n if n <= MAX_AMBIGUOUS => {
                let recv = r.receiver.clone().unwrap_or_default();
                let label = if recv.is_empty() { r.name.clone() } else { format!("{recv}.{}", r.name) };
                self.emit_unresolved_call(r, &label, format!("{n} potential targets for {label}() (dynamic receiver)"), &cands);
            }
            _ => {}
        }
    }

    fn construct(&mut self, r: &Reference) {
        let file = self.file;
        // JSX components can be functions or classes; `new` targets types.
        if r.kind == RefKind::Jsx {
            if let Some(recv) = &r.receiver {
                if let Some((m, c, ev)) = self.index.resolve_imported(file, recv) {
                    let mf = self.index.syms[m].file.clone();
                    if let Some(s) = self.index.exported_symbol(&mf, &r.name, 0) {
                        self.emit_call(r, s, c, format!("JSX <{recv}.{}> via {ev}", r.name), "jsx-resolver");
                    }
                }
                return;
            }
            match self.index.resolve_value(&r.name, file, &r.enclosing) {
                Resolved::One(t, c, ev) => self.emit_call(r, t, c, format!("JSX <{}> {ev}", r.name), "jsx-resolver"),
                _ => {
                    if let Resolved::One(t, c, ev) = self.ty(&r.name, r) {
                        self.emit_call(r, t, c, format!("JSX <{}> {ev}", r.name), "jsx-resolver");
                    }
                }
            }
            return;
        }
        match self.ty(&r.name, r) {
            Resolved::One(t, c, ev) => self.emit_call(r, t, c, format!("new {} ({ev})", r.name), "call-resolver"),
            Resolved::Many(c) if c.len() <= MAX_AMBIGUOUS => {
                self.emit_unresolved_call(r, &r.name.clone(), format!("{} candidate types named {}", c.len(), r.name), &c);
            }
            _ => {}
        }
    }

    fn throw(&mut self, r: &Reference) {
        let mut meta = json!({});
        if let Some(m) = &r.detail {
            meta["message"] = json!(m);
        }
        if !r.catch_types.is_empty() {
            meta["try"] = json!(r.catch_types);
        }
        match self.ty(&r.name, r) {
            Resolved::One(t, c, ev) => {
                let e = Edge::new(
                    &r.enclosing,
                    self.file,
                    Some(self.target(t)),
                    relation::THROWS,
                    c,
                    format!("throws {} ({ev})", r.name),
                    "error-flow",
                    r.line,
                )
                .with_meta(meta);
                self.push(e);
            }
            _ => {
                meta["external"] = json!(true);
                let e = Edge::unresolved(
                    &r.enclosing,
                    self.file,
                    relation::THROWS,
                    &r.name,
                    format!("throws external/builtin {}", r.name),
                    "error-flow",
                    r.line,
                )
                .with_meta(meta);
                self.push(e);
            }
        }
    }

    fn var_access(&mut self, r: &Reference) {
        if r.kind == RefKind::Read && self.method_group(r) {
            return;
        }
        let rel = if r.kind == RefKind::Write { relation::WRITES_VAR } else { relation::READS_VAR };
        let is_var = |idx: &SymbolIndex, i: usize| idx.syms[i].sym.kind.is_variable();
        let file = self.file;
        let found: Option<(usize, Confidence, String)> = match r.receiver.as_deref() {
            None => match self.index.resolve_value(&r.name, file, &r.enclosing) {
                Resolved::One(t, c, ev) if is_var(self.index, t) => Some((t, c, ev)),
                // A class / enum / interface used as a value (`providers: [Svc]`,
                // `x instanceof Foo`, `@UseGuards(JwtGuard)`).
                Resolved::One(t, c, ev) if r.kind == RefKind::Read && self.index.syms[t].sym.kind.is_type() => {
                    let e = Edge::new(
                        &r.enclosing,
                        file,
                        Some(self.target(t)),
                        relation::USES_TYPE,
                        c,
                        format!("{} used as a value ({ev})", r.name),
                        "type-use",
                        r.line,
                    );
                    self.push(e);
                    None
                }
                _ => None,
            },
            Some("this") | Some("self") | Some("cls") => self.enclosing_type(&r.enclosing).and_then(|t| {
                self.index
                    .members_named(t, &r.name)
                    .into_iter()
                    .find(|m| is_var(self.index, *m))
                    .map(|m| (m, Confidence::Exact, "member of enclosing class".to_string()))
            }),
            Some(recv) => {
                let typed = r.receiver_type.clone().or_else(|| {
                    if recv.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false) && !recv.contains('.') {
                        Some(recv.to_string())
                    } else {
                        None
                    }
                });
                match typed.map(|t| self.ref_type(&t, r, 0)) {
                    Some(TypeOf::Project(t, c, ev)) => self
                        .index
                        .members_named(t, &r.name)
                        .into_iter()
                        .find(|m| is_var(self.index, *m))
                        .map(|m| (m, c, format!("typed receiver {recv} ({ev})"))),
                    _ => None,
                }
            }
        };
        if let Some((t, c, ev)) = found {
            let e = Edge::new(&r.enclosing, file, Some(self.target(t)), rel, c, ev, "var-tracker", r.line);
            self.push(e);
        }
    }
}

/// True when a callable with these parameters can be invoked with `argc` arguments.
fn accepts_args(params: &Option<codegraph_core::ParamInfo>, argc: u32) -> bool {
    match params {
        None => true,
        Some(p) => argc >= p.required && (p.variadic || argc <= p.total),
    }
}

fn looks_like_interface(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() > 2 && b[0] == b'I' && b[1].is_ascii_uppercase()
}

/// NestJS: `constructor(@Inject('TOKEN') private readonly field: Type)` → token for `field`.
fn inject_token(index: &SymbolIndex, class_idx: usize, field: &str) -> Option<String> {
    let cls_id = &index.syms[class_idx].sym.id;
    let ctor = index.children.get(cls_id)?.iter().find(|k| index.syms[**k].sym.kind == SymbolKind::Constructor)?;
    let sig = &index.syms[*ctor].sym.signature;
    let mut rest = sig.as_str();
    while let Some(pos) = rest.find("@Inject(") {
        let after = &rest[pos + 8..];
        let q = after.find(['"', '\''])?;
        let quote = after.as_bytes()[q] as char;
        let tok_start = &after[q + 1..];
        let tok_end = tok_start.find(quote)?;
        let token = &tok_start[..tok_end];
        let tail = &tok_start[tok_end + 1..];
        let tail = tail.trim_start_matches(')').trim_start();
        let words: Vec<&str> = tail.split(|c: char| c.is_whitespace() || c == ':').filter(|w| !w.is_empty()).collect();
        let pname = words.iter().find(|w| !matches!(**w, "private" | "public" | "protected" | "readonly"));
        if pname == Some(&field) {
            return Some(token.to_string());
        }
        rest = tail;
    }
    None
}

pub fn resolve_file(index: &SymbolIndex, file: &str, refs: &[Reference]) -> Vec<Edge> {
    FileResolver::new(index, file).resolve(refs)
}
