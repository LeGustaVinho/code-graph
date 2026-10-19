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
                match self.index.resolve_type(raw, self.file) {
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
            let ctor = self
                .index
                .children
                .get(&self.index.syms[t].sym.id)
                .and_then(|kids| kids.iter().copied().find(|k| self.index.syms[*k].sym.kind == SymbolKind::Constructor));
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
                self.emit_unresolved_call(r, &sym, format!("{n} potential implementations of {sym}"), &targets);
            }
        }
    }

    fn call_on_type(&mut self, r: &Reference, t: usize, conf: Confidence, ev: String, field: Option<&str>) -> bool {
        let methods: Vec<usize> =
            self.index.members_named(t, &r.name).into_iter().filter(|m| self.index.syms[*m].sym.kind.is_callable()).collect();
        if methods.is_empty() {
            return false;
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
        }
        true
    }

    fn call(&mut self, r: &Reference) {
        let file = self.file;
        match r.receiver.as_deref() {
            None => {
                // C# implicit `this`: methods of the enclosing type.
                if self.lang == Language::CSharp {
                    if let Some(t) = self.enclosing_type(&r.enclosing) {
                        let tn = self.index.syms[t].sym.name.clone();
                        if self.call_on_type(r, t, Confidence::Exact, format!("same-class method of {tn}"), None) {
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
                if let Resolved::One(t, c, ev) = self.index.resolve_type(&r.name, file) {
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
                let field = recv_l.strip_prefix("this.").or_else(|| recv_l.strip_prefix("self."));
                // Typed receiver (declared / inferred static type).
                if let Some(rt) = &r.receiver_type {
                    match self.index.resolve_type(rt, file) {
                        Resolved::One(t, c, ev) => {
                            let tn = self.index.syms[t].sym.name.clone();
                            let how = format!("typed receiver '{recv_l}: {tn}' ({ev})");
                            if self.call_on_type(r, t, c, how, field.or(Some(recv_l))) {
                                return;
                            }
                            return; // method inherited from an external type
                        }
                        Resolved::Many(_) => {}
                        Resolved::None => return, // external type (HttpClient, ILogger, ...)
                    }
                }
                let head = recv_l.split('.').next().unwrap_or(recv_l);
                // Namespace / module import: `utils.hash()`, `models.Item(...)`.
                if !recv_l.contains('(') {
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
                                }
                            }
                            None => {} // external package: not part of the graph
                        }
                        return;
                    }
                }
                // Static call on a type: `OrderService.Create()`, `Item.query()`.
                if !recv_l.contains('.') && !recv_l.contains('(') && recv_l.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
                {
                    match self.index.resolve_type(recv_l, file) {
                        Resolved::One(t, c, ev) => {
                            self.call_on_type(r, t, c, format!("static type reference {recv_l} ({ev})"), None);
                            return;
                        }
                        Resolved::None => return,
                        Resolved::Many(_) => {}
                    }
                }
                self.global_by_name(r, true);
            }
        }
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
                    if let Resolved::One(t, c, ev) = self.index.resolve_type(&r.name, file) {
                        self.emit_call(r, t, c, format!("JSX <{}> {ev}", r.name), "jsx-resolver");
                    }
                }
            }
            return;
        }
        match self.index.resolve_type(&r.name, file) {
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
        match self.index.resolve_type(&r.name, self.file) {
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
        let rel = if r.kind == RefKind::Write { relation::WRITES_VAR } else { relation::READS_VAR };
        let is_var = |idx: &SymbolIndex, i: usize| idx.syms[i].sym.kind.is_variable();
        let file = self.file;
        let found: Option<(usize, Confidence, String)> = match r.receiver.as_deref() {
            None => match self.index.resolve_value(&r.name, file, &r.enclosing) {
                Resolved::One(t, c, ev) if is_var(self.index, t) => Some((t, c, ev)),
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
                match typed.map(|t| self.index.resolve_type(&t, file)) {
                    Some(Resolved::One(t, c, ev)) => self
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
