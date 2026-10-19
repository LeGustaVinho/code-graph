//! Project-wide derived relations, recomputed on every indexing run:
//! RESOLVES_DI, HTTP_CALLS (fullstack route linker), TESTS, CAUGHT_BY and USES_LITERAL.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use codegraph_core::extract::clean_type;
use codegraph_core::routes::normalize_route;
use codegraph_core::{GlobalHandler, HttpCall, RouteDecl, Symbol, SymbolKind};
use serde_json::json;

use crate::index::{dirname, Resolved, SymbolIndex};
use crate::{file_name, relation, Confidence, Edge};

// ---------------------------------------------------------------------------
// Dependency injection
// ---------------------------------------------------------------------------

pub fn di_edges(index: &SymbolIndex) -> Vec<Edge> {
    let mut out = Vec::new();
    let mut keys: Vec<&(String, String)> = index.di.keys().collect();
    keys.sort();
    for entries in keys.into_iter().map(|k| &index.di[k]) {
        for e in entries {
            let reg = &e.reg;
            let file = &e.file;
            let service_node = match index.resolve_type(&reg.service, file) {
                Resolved::One(s, _, _) => Some(s),
                _ => None,
            };
            let source_id = service_node.map(|s| index.syms[s].sym.id.clone()).unwrap_or_else(|| reg.enclosing.clone());
            let source_file = service_node.map(|s| index.syms[s].file.clone()).unwrap_or_else(|| file.clone());
            let Some(imp) = &reg.implementation else { continue };
            let target = match index.resolve_type(imp, file) {
                Resolved::One(t, _, _) => Some(t),
                _ => match index.resolve_value(imp, file, &reg.enclosing) {
                    Resolved::One(t, _, _) => Some(t),
                    _ => None,
                },
            };
            let evidence = if reg.framework == "fastapi" {
                format!("FastAPI Depends({imp}) for parameter '{}' in {}:{}", reg.service, file_name(file), reg.line)
            } else {
                format!("DI {} registration {} -> {} in {}:{}", reg.lifetime, reg.service, imp, file_name(file), reg.line)
            };
            let meta = json!({ "lifetime": reg.lifetime, "framework": reg.framework, "service": reg.service, "registered_at": format!("{}:{}", file, reg.line) });
            match target {
                Some(t) => {
                    let (fastapi_src, fastapi_file) = if reg.framework == "fastapi" {
                        (reg.enclosing.clone(), file.clone())
                    } else {
                        (source_id.clone(), source_file.clone())
                    };
                    out.push(
                        Edge::new(
                            &fastapi_src,
                            &fastapi_file,
                            Some((&index.syms[t].sym.id, &index.syms[t].file)),
                            relation::RESOLVES_DI,
                            Confidence::Exact,
                            evidence,
                            "di-resolver",
                            reg.line,
                        )
                        .with_meta(meta)
                        .derived(),
                    );
                }
                None => out.push(
                    Edge::unresolved(
                        &source_id,
                        &source_file,
                        relation::RESOLVES_DI,
                        imp,
                        format!("{evidence} (implementation not found in project)"),
                        "di-resolver",
                        reg.line,
                    )
                    .with_meta(meta)
                    .derived(),
                ),
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Fullstack route linker
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RouteRow {
    pub file: String,
    pub decl: RouteDecl,
}

#[derive(Clone, Debug)]
pub struct HttpCallRow {
    pub file: String,
    pub call: HttpCall,
}

#[derive(Clone, Debug)]
pub struct ResolvedRoute {
    pub file: String,
    pub handler_id: String,
    pub http_method: String,
    pub path: String,
    pub normalized: String,
    pub framework: String,
    pub line: u32,
}

/// Resolves `ref:Name|fallback` handler references produced by extractors.
pub fn resolve_route_handler(index: &SymbolIndex, file: &str, handler: &str) -> String {
    if let Some(rest) = handler.strip_prefix("ref:") {
        let (name, fallback) = rest.split_once('|').unwrap_or((rest, ""));
        if let Resolved::One(t, _, _) = index.resolve_value(name, file, fallback) {
            return index.syms[t].sym.id.clone();
        }
        if let Some(ids) = index.by_file.get(file) {
            if let Some(i) = ids.iter().find(|i| index.syms[**i].sym.name == name && index.syms[**i].sym.kind.is_callable()) {
                return index.syms[*i].sym.id.clone();
            }
        }
        return fallback.to_string();
    }
    handler.to_string()
}

pub fn resolve_routes(index: &SymbolIndex, routes: &[RouteRow], prefixes: &[(String, String)]) -> Vec<ResolvedRoute> {
    let mut out = Vec::new();
    for r in routes {
        let handler_id = resolve_route_handler(index, &r.file, &r.decl.handler);
        let normalized = normalize_route(&r.decl.path);
        out.push(ResolvedRoute {
            file: r.file.clone(),
            handler_id: handler_id.clone(),
            http_method: r.decl.http_method.clone(),
            path: r.decl.path.clone(),
            normalized: normalized.clone(),
            framework: r.decl.framework.clone(),
            line: r.decl.line,
        });
        // Global prefixes (NestJS setGlobalPrefix) apply to routes of the same app (same top directory).
        if r.decl.framework == "nestjs" {
            for (pfile, p) in prefixes {
                if same_app(pfile, &r.file) {
                    let full = format!("/{}/{}", p.trim_matches('/'), r.decl.path.trim_start_matches('/'));
                    out.push(ResolvedRoute {
                        file: r.file.clone(),
                        handler_id: handler_id.clone(),
                        http_method: r.decl.http_method.clone(),
                        path: full.clone(),
                        normalized: normalize_route(&full),
                        framework: r.decl.framework.clone(),
                        line: r.decl.line,
                    });
                }
            }
        }
    }
    out
}

fn same_app(a: &str, b: &str) -> bool {
    // Heuristic: share the directory that contains `src/`, or the first two path segments.
    let root = |p: &str| -> String {
        if let Some(i) = p.find("/src/") {
            return p[..i].to_string();
        }
        p.split('/').take(2).collect::<Vec<_>>().join("/")
    };
    root(a) == root(b)
}

fn methods_compatible(call: &str, route: &str) -> bool {
    call == route || route == "ANY" || call.is_empty() || route.is_empty()
}

fn dedupe_routes(v: Vec<&ResolvedRoute>) -> Vec<&ResolvedRoute> {
    let mut seen = HashSet::new();
    v.into_iter().filter(|r| seen.insert(r.handler_id.clone())).collect()
}

pub fn http_edges(index: &SymbolIndex, routes: &[ResolvedRoute], calls: &[HttpCallRow]) -> Vec<Edge> {
    let mut out = Vec::new();
    for c in calls {
        let norm = normalize_route(&c.call.url);
        // `${API_BASE}/orders` → match on the suffix after leading parameters.
        let (suffix_mode, needle) = if norm.starts_with("{}") {
            let stripped: String = norm.split('/').skip_while(|s| *s == "{}").collect::<Vec<_>>().join("/");
            (true, format!("/{stripped}"))
        } else {
            (false, norm.clone())
        };
        if needle == "/" {
            continue;
        }
        let call_family = index.family_of_file(&c.file);
        let mut exact: Vec<&ResolvedRoute> = Vec::new();
        let mut suffix: Vec<&ResolvedRoute> = Vec::new();
        for r in routes {
            if r.handler_id.is_empty() || !methods_compatible(&c.call.http_method, &r.http_method) {
                continue;
            }
            if !suffix_mode && r.normalized == needle {
                exact.push(r);
            } else if r.normalized.ends_with(&needle) && (suffix_mode || needle.matches('/').count() >= 2) {
                suffix.push(r);
            }
        }
        // Collapse duplicates (same handler via several prefixed variants).
        let exact = dedupe_routes(exact);
        let suffix = dedupe_routes(suffix);
        let (cands, kind) = if !exact.is_empty() { (exact, "route match") } else { (suffix, "route suffix match") };
        let meta = |r: Option<&ResolvedRoute>| {
            json!({
                "method": c.call.http_method,
                "url": c.call.url,
                "client": c.call.client,
                "route": r.map(|r| format!("{} {}", r.http_method, r.path)),
                "framework": r.map(|r| r.framework.clone()),
                "cross_language": r.map(|r| index.family_of_file(&r.file) != call_family),
            })
        };
        match cands.len() {
            0 => {}
            1 => {
                let r = cands[0];
                let tf = index.get(&r.handler_id).map(|s| s.file.clone()).unwrap_or_else(|| r.file.clone());
                let ev = format!(
                    "{kind} '{} {}' ↔ {} {} ({}:{})",
                    c.call.http_method,
                    c.call.url,
                    r.http_method,
                    r.path,
                    file_name(&r.file),
                    r.line
                );
                out.push(
                    Edge::new(
                        &c.call.enclosing,
                        &c.file,
                        Some((&r.handler_id, &tf)),
                        relation::HTTP_CALLS,
                        Confidence::Inferred,
                        ev,
                        "fullstack-route-linker",
                        c.call.line,
                    )
                    .with_meta(meta(Some(r)))
                    .derived(),
                );
            }
            n => {
                let mut cands = cands;
                cands.sort_by_key(|r| (r.file.clone(), r.line, r.http_method.clone(), r.path.clone(), r.handler_id.clone()));
                let mut m = meta(None);
                m["candidates"] =
                    json!(cands.iter().map(|r| format!("{} {} {}:{}", r.http_method, r.path, r.file, r.line)).collect::<Vec<_>>());
                m["candidate_ids"] = json!(cands.iter().map(|r| r.handler_id.clone()).collect::<Vec<_>>());
                out.push(
                    Edge::unresolved(
                        &c.call.enclosing,
                        &c.file,
                        relation::HTTP_CALLS,
                        &format!("{} {}", c.call.http_method, c.call.url),
                        format!("{n} candidate backend routes for {} {}", c.call.http_method, c.call.url),
                        "fullstack-route-linker",
                        c.call.line,
                    )
                    .with_meta(m)
                    .derived(),
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Test linker
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CallRow {
    pub source_id: String,
    pub target_id: String,
    pub confidence: Confidence,
    pub line: u32,
    pub try_types: Vec<String>,
    pub source_file: String,
}

pub fn test_edges(index: &SymbolIndex, calls: &[CallRow]) -> Vec<Edge> {
    let mut out = Vec::new();
    let is_test = |id: &str| index.get(id).map(|s| s.sym.is_test).unwrap_or(false);
    let mut seen = HashSet::new();
    for c in calls {
        if !is_test(&c.source_id) || is_test(&c.target_id) {
            continue;
        }
        let Some(t) = index.get(&c.target_id) else { continue };
        if t.sym.kind == SymbolKind::Module {
            continue;
        }
        if !seen.insert((c.source_id.clone(), c.target_id.clone())) {
            continue;
        }
        out.push(
            Edge::new(
                &c.source_id,
                &c.source_file,
                Some((&c.target_id, &t.file)),
                relation::TESTS,
                c.confidence,
                format!("test invokes {} directly", t.sym.name),
                "test-linker",
                c.line,
            )
            .derived(),
        );
    }
    // Naming conventions.
    for s in &index.syms {
        if !s.sym.is_test {
            continue;
        }
        let target_name = match s.sym.kind {
            SymbolKind::Class => s
                .sym
                .name
                .strip_suffix("Tests")
                .or_else(|| s.sym.name.strip_suffix("Test"))
                .or_else(|| s.sym.name.strip_suffix("Spec"))
                .or_else(|| s.sym.name.strip_prefix("Test")),
            _ => None,
        };
        if let Some(tn) = target_name.filter(|t| !t.is_empty()) {
            if let Resolved::One(t, _, ev) = index.resolve_type(tn, &s.file) {
                if seen.insert((s.sym.id.clone(), index.syms[t].sym.id.clone())) {
                    out.push(
                        Edge::new(
                            &s.sym.id,
                            &s.file,
                            Some((&index.syms[t].sym.id, &index.syms[t].file)),
                            relation::TESTS,
                            Confidence::Inferred,
                            format!("naming convention {} → {} ({ev})", s.sym.name, tn),
                            "test-linker",
                            s.sym.start_line,
                        )
                        .derived(),
                    );
                }
            }
        }
        if s.sym.kind == SymbolKind::Module {
            if let Some(target) = production_module_for_test(index, &s.file) {
                let t = &index.syms[target];
                if seen.insert((s.sym.id.clone(), t.sym.id.clone())) {
                    out.push(
                        Edge::new(
                            &s.sym.id,
                            &s.file,
                            Some((&t.sym.id, &t.file)),
                            relation::TESTS,
                            Confidence::Inferred,
                            format!("test file naming convention {} → {}", file_name(&s.file), file_name(&t.file)),
                            "test-linker",
                            1,
                        )
                        .derived(),
                    );
                }
            }
        }
    }
    out
}

fn production_module_for_test(index: &SymbolIndex, test_file: &str) -> Option<usize> {
    let name = file_name(test_file);
    let stem = if let Some(p) = name.find(".test.").or_else(|| name.find(".spec.")) {
        name[..p].to_string()
    } else if let Some(rest) = name.strip_prefix("test_") {
        rest.trim_end_matches(".py").to_string()
    } else if let Some(rest) = name.strip_suffix("_test.py") {
        rest.to_string()
    } else {
        return None;
    };
    let dir = dirname(test_file);
    let mut cands: Vec<usize> = index
        .module_of_file
        .iter()
        .filter(|(f, _)| {
            f.as_str() != test_file && {
                let n = file_name(f);
                n.split('.').next() == Some(stem.as_str())
            }
        })
        .map(|(_, i)| *i)
        .filter(|i| !index.syms[*i].sym.is_test)
        .collect();
    cands.sort_by_key(|i| {
        let f = &index.syms[*i].file;
        // Prefer the same directory, then the parent (for __tests__ folders), then shortest path.
        let d = dirname(f);
        (
            if d == dir {
                0
            } else if dir.starts_with(d) {
                1
            } else {
                2
            },
            f.len(),
            f.clone(),
        )
    });
    let first = *cands.first()?;
    let d = dirname(&index.syms[first].file);
    if cands.len() == 1 || d == dir || dir.starts_with(d) {
        Some(first)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Error flow (throw → catch)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ThrowRow {
    pub source_id: String,
    pub source_file: String,
    pub exception: String,
    pub line: u32,
    pub try_types: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct HandlerRow {
    pub file: String,
    pub handler: GlobalHandler,
}

const ROOT_EXCEPTIONS: &[&str] = &["Exception", "System.Exception", "BaseException", "Error", "Throwable", "*"];

/// Names of `exception` and all its ancestors (project types + declared external bases).
pub fn exception_ancestors(index: &SymbolIndex, exception: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut queue = VecDeque::from([clean_type(exception)]);
    while let Some(n) = queue.pop_front() {
        if !out.insert(n.clone()) || out.len() > 32 {
            continue;
        }
        if let Some(ids) = index.by_name.get(&n) {
            for i in ids {
                let s = &index.syms[*i].sym;
                if s.kind.is_type() {
                    for b in &s.bases {
                        queue.push_back(clean_type(b));
                    }
                }
            }
        }
    }
    out
}

pub fn catches(catch_type: &str, ancestors: &HashSet<String>) -> bool {
    let c = clean_type(catch_type);
    ROOT_EXCEPTIONS.contains(&c.as_str()) || ancestors.contains(&c)
}

pub fn caught_by_edges(index: &SymbolIndex, throws: &[ThrowRow], calls: &[CallRow], handlers: &[HandlerRow]) -> Vec<Edge> {
    let mut callers: HashMap<&str, Vec<&CallRow>> = HashMap::new();
    for c in calls {
        callers.entry(c.target_id.as_str()).or_default().push(c);
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for t in throws {
        let anc = exception_ancestors(index, &t.exception);
        // Caught inside the throwing function.
        if let Some(ct) = t.try_types.iter().find(|ct| catches(ct, &anc)) {
            if seen.insert((t.source_id.clone(), t.source_id.clone(), t.exception.clone())) {
                out.push(
                    Edge::new(
                        &t.source_id,
                        &t.source_file,
                        Some((&t.source_id, &t.source_file)),
                        relation::CAUGHT_BY,
                        Confidence::Exact,
                        format!("{} thrown at {}:{} caught locally (catch {ct})", t.exception, file_name(&t.source_file), t.line),
                        "error-flow",
                        t.line,
                    )
                    .with_meta(json!({ "exception": t.exception, "hops": 0, "catch": ct }))
                    .derived(),
                );
            }
            continue;
        }
        // Propagate up the call graph.
        let mut queue: VecDeque<(String, u32, Confidence)> = VecDeque::from([(t.source_id.clone(), 0, Confidence::Exact)]);
        let mut visited: HashSet<String> = HashSet::from([t.source_id.clone()]);
        let mut reached_root = false;
        while let Some((node, depth, conf)) = queue.pop_front() {
            let Some(cs) = callers.get(node.as_str()) else {
                reached_root = true;
                continue;
            };
            for c in cs {
                let hop_conf = conf.weakest(c.confidence);
                if let Some(ct) = c.try_types.iter().find(|ct| catches(ct, &anc)) {
                    if seen.insert((t.source_id.clone(), c.source_id.clone(), t.exception.clone())) {
                        let catcher = index.get(&c.source_id).map(|s| s.sym.qualified_name.clone()).unwrap_or_default();
                        let ev = format!(
                            "{} thrown at {}:{} propagates {} hop(s) to {} where the call at {}:{} is wrapped in catch ({ct})",
                            t.exception,
                            file_name(&t.source_file),
                            t.line,
                            depth + 1,
                            catcher,
                            file_name(&c.source_file),
                            c.line
                        );
                        let conf = if depth == 0 { hop_conf } else { hop_conf.weakest(Confidence::Inferred) };
                        out.push(
                            Edge::new(
                                &t.source_id,
                                &t.source_file,
                                Some((&c.source_id, &c.source_file)),
                                relation::CAUGHT_BY,
                                conf,
                                ev,
                                "error-flow",
                                c.line,
                            )
                            .with_meta(json!({ "exception": t.exception, "hops": depth + 1, "catch": ct, "throw_line": t.line }))
                            .derived(),
                        );
                    }
                    continue;
                }
                if depth < 8 && visited.insert(c.source_id.clone()) {
                    queue.push_back((c.source_id.clone(), depth + 1, hop_conf));
                }
            }
        }
        if reached_root {
            let fam = index.family_of_file(&t.source_file);
            // Only the handlers of the same application: nearest by directory.
            let cands: Vec<&HandlerRow> = handlers
                .iter()
                .filter(|h| index.family_of_file(&h.file) == fam && (h.handler.exception == "*" || catches(&h.handler.exception, &anc)))
                .collect();
            let best = cands.iter().map(|h| shared_dirs(&h.file, &t.source_file)).max().unwrap_or(0);
            for h in cands {
                if cands_len_guard(best, shared_dirs(&h.file, &t.source_file)) {
                    continue;
                }
                if seen.insert((t.source_id.clone(), h.handler.handler.clone(), t.exception.clone())) {
                    let ev = format!(
                        "{} thrown at {}:{} is not caught in the call chain; handled by global {} exception handler at {}:{}",
                        t.exception,
                        file_name(&t.source_file),
                        t.line,
                        h.handler.framework,
                        file_name(&h.file),
                        h.handler.line
                    );
                    out.push(
                        Edge::new(
                            &t.source_id,
                            &t.source_file,
                            Some((&h.handler.handler, &h.file)),
                            relation::CAUGHT_BY,
                            Confidence::Inferred,
                            ev,
                            "error-flow",
                            h.handler.line,
                        )
                        .with_meta(json!({ "exception": t.exception, "global_handler": true, "throw_line": t.line }))
                        .derived(),
                    );
                }
            }
        }
    }
    out
}

/// Number of leading directory segments shared by two file paths.
fn shared_dirs(a: &str, b: &str) -> usize {
    let da: Vec<&str> = dirname(a).split('/').filter(|s| !s.is_empty()).collect();
    let db: Vec<&str> = dirname(b).split('/').filter(|s| !s.is_empty()).collect();
    da.iter().zip(db.iter()).take_while(|(x, y)| x == y).count()
}

/// True when a handler is farther from the thrower than the nearest candidate.
fn cands_len_guard(best: usize, this: usize) -> bool {
    this < best
}

// ---------------------------------------------------------------------------
// Shared literals
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct LiteralRow {
    pub value: String,
    pub file: String,
    pub line: u32,
    pub enclosing: String,
}

/// Literal nodes for string values shared by two or more symbols, plus USES_LITERAL edges.
pub fn literal_graph(literals: &[LiteralRow], max_nodes: usize) -> (Vec<(String, Symbol)>, Vec<Edge>) {
    let mut groups: BTreeMap<&str, Vec<&LiteralRow>> = BTreeMap::new();
    for l in literals {
        let v = l.value.as_str();
        if v.chars().count() < 4 || v.chars().count() > 120 || !v.chars().any(|c| c.is_alphabetic()) || v.contains('\n') {
            continue;
        }
        groups.entry(v).or_default().push(l);
    }
    let mut shared: Vec<(&str, Vec<&LiteralRow>)> =
        groups.into_iter().filter(|(_, occ)| occ.iter().map(|o| o.enclosing.as_str()).collect::<HashSet<_>>().len() >= 2).collect();
    shared.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    shared.truncate(max_nodes);
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (value, occ) in shared {
        let first = occ[0];
        let id = format!("lit_{}", codegraph_core::hash::short_hash(value));
        let sym = Symbol {
            id: id.clone(),
            kind: SymbolKind::Literal,
            name: codegraph_core::extract::truncate(value, 60),
            qualified_name: format!("literal:{value}"),
            signature: format!("\"{}\" ({} occurrences)", codegraph_core::extract::truncate(value, 80), occ.len()),
            start_line: first.line,
            end_line: first.line,
            parent_id: None,
            visibility: "public".into(),
            is_exported: false,
            is_static: true,
            is_async: false,
            is_abstract: false,
            capabilities: Default::default(),
            complexity: 0,
            is_entrypoint: false,
            entry_kind: None,
            is_test: false,
            docstring: None,
            attrs: Vec::new(),
            bases: Vec::new(),
            type_ref: Some("string".into()),
            value_text: Some(value.to_string()),
            params: None,
            body_hash: String::new(),
            namespace: None,
        };
        nodes.push((first.file.clone(), sym));
        let mut seen = HashSet::new();
        for o in occ {
            if !seen.insert((o.enclosing.clone(), o.line)) {
                continue;
            }
            edges.push(
                Edge::new(
                    &o.enclosing,
                    &o.file,
                    Some((&id, &first.file)),
                    relation::USES_LITERAL,
                    Confidence::Exact,
                    format!("uses literal \"{}\" at {}:{}", codegraph_core::extract::truncate(value, 40), file_name(&o.file), o.line),
                    "literal-linker",
                    o.line,
                )
                .derived(),
            );
        }
    }
    (nodes, edges)
}
