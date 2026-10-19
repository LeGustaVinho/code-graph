//! 5.3 Structural navigation, tracing & bridges.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use codegraph_core::extract::clean_type;
use codegraph_core::routes::normalize_route;
use codegraph_core::SymbolKind;
use codegraph_graph::query;
use codegraph_graph::store::{node_from_row, NODE_COLS};
use codegraph_graph::{EdgeRow, Node};
use codegraph_semantic::Confidence;
use rusqlite::{params, Connection};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, edge_line, node_line, short_evidence};
use super::{nodes_map, resolve_symbol, wants_json, Ctx, Out, ToolError, ToolResult};

pub const FLOW_RELATIONS: &[&str] = &["CALLS", "HTTP_CALLS", "RESOLVES_DI"];

/// BFS shortest path (exact edges explored first). Returns the path and unresolved
/// edges met on the way (possible dynamic hops).
pub fn shortest_path(
    conn: &Connection,
    from: Vec<String>,
    to: &HashSet<String>,
    max_depth: usize,
    rels: &[&str],
) -> anyhow::Result<(Option<Vec<EdgeRow>>, Vec<EdgeRow>)> {
    let mut prev: HashMap<String, EdgeRow> = HashMap::new();
    let mut visited: HashSet<String> = from.iter().cloned().collect();
    let mut queue: VecDeque<(String, usize)> = from.into_iter().map(|f| (f, 0)).collect();
    let mut unresolved = Vec::new();
    while let Some((id, depth)) = queue.pop_front() {
        if to.contains(&id) {
            let mut path = Vec::new();
            let mut cur = id;
            while let Some(e) = prev.get(&cur) {
                path.push(e.clone());
                cur = e.source_id.clone();
            }
            path.reverse();
            return Ok((Some(path), unresolved));
        }
        if depth >= max_depth {
            continue;
        }
        let mut out = query::edges_out(conn, &id, rels)?;
        out.sort_by_key(|e| e.confidence);
        for e in out {
            match &e.target_id {
                Some(t) => {
                    if visited.insert(t.clone()) {
                        prev.insert(t.clone(), e.clone());
                        queue.push_back((t.clone(), depth + 1));
                    }
                }
                None => {
                    if unresolved.len() < 50 {
                        unresolved.push(e);
                    }
                }
            }
        }
    }
    Ok((None, unresolved))
}

fn expand_type(conn: &Connection, n: &Node) -> anyhow::Result<Vec<String>> {
    let mut ids = vec![n.sym.id.clone()];
    if n.sym.kind.is_type() || n.sym.kind == SymbolKind::Module {
        ids.extend(query::children(conn, &n.sym.id)?.into_iter().filter(|c| c.sym.kind.is_callable()).map(|c| c.sym.id));
    }
    Ok(ids)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceParams {
    /// Start symbol (id, name, `Type.member` or `file:line`).
    pub source: String,
    /// Destination symbol.
    pub target: String,
    /// Maximum hops (default 8).
    pub max_depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn trace_execution_path(ctx: &Ctx, p: TraceParams) -> ToolResult {
    let s = resolve_symbol(ctx.conn, &p.source)?;
    let t = resolve_symbol(ctx.conn, &p.target)?;
    let depth = p.max_depth.unwrap_or(8).clamp(1, 20);
    let to: HashSet<String> = expand_type(ctx.conn, &t)?.into_iter().collect();
    let (path, unresolved) = shortest_path(ctx.conn, expand_type(ctx.conn, &s)?, &to, depth, FLOW_RELATIONS)?;
    let mut out = Out::new();
    match path {
        Some(path) if !path.is_empty() || s.sym.id == t.sym.id => {
            let weakest = path.iter().map(|e| e.confidence).max().unwrap_or(Confidence::Exact);
            out.h(format!("Path {} → {}: {} hop(s), weakest link: {}", display(&s), display(&t), path.len(), weakest.as_str()));
            let nodes = nodes_map(ctx.conn, path.iter().flat_map(|e| std::iter::once(e.source_id.clone()).chain(e.target_id.clone())))?;
            let json = wants_json(&p.format);
            for (i, e) in path.iter().enumerate() {
                if json {
                    out.item(json!({ "hop": i + 1, "from": e.source_id, "to": e.target_id, "site": format!("{}:{}", e.source_file, e.line), "relation": e.relation, "confidence": e.confidence.as_str(), "evidence": e.evidence }).to_string());
                } else {
                    out.item(format!("{}. {}", i + 1, edge_line(e, &nodes)));
                }
            }
        }
        _ => {
            out.h(format!("No resolved path from {} to {} within {depth} hop(s)", display(&s), display(&t)));
            if !unresolved.is_empty() {
                out.h("Unresolved (dynamic) calls met while searching — possible missing hops:");
                let nodes = nodes_map(ctx.conn, unresolved.iter().map(|e| e.source_id.clone()))?;
                for e in unresolved.iter().take(8) {
                    out.item(edge_line(e, &nodes));
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Call trees
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn tree(
    conn: &Connection,
    id: &str,
    callees: bool,
    depth: usize,
    max_depth: usize,
    rels: &[&str],
    visited: &mut HashSet<String>,
    lines: &mut Vec<String>,
    with_caps: bool,
) -> anyhow::Result<()> {
    if depth >= max_depth || lines.len() > 400 {
        return Ok(());
    }
    let edges = if callees { query::edges_out(conn, id, rels)? } else { query::edges_in(conn, id, rels)? };
    let others: Vec<String> = edges.iter().filter_map(|e| if callees { e.target_id.clone() } else { Some(e.source_id.clone()) }).collect();
    let nodes = query::nodes_by_ids(conn, &others)?;
    let indent = "  ".repeat(depth + 1);
    let arrow = if callees { "→" } else { "←" };
    let mut seen_here = HashSet::new();
    for e in edges {
        let other = if callees { e.target_id.clone() } else { Some(e.source_id.clone()) };
        match other.and_then(|o| nodes.get(&o).cloned()) {
            Some(n) => {
                if n.sym.id == id || !seen_here.insert(n.sym.id.clone()) {
                    continue;
                }
                let site = if callees { format!("@{}", e.line) } else { format!("@{}:{}", e.source_file, e.line) };
                let rel = if e.relation == "CALLS" { String::new() } else { format!(" {}", e.relation) };
                let caps = if with_caps && !n.sym.capabilities.is_empty() {
                    format!(" {{{}}}", n.sym.capabilities.iter().cloned().collect::<Vec<_>>().join(","))
                } else {
                    String::new()
                };
                let recursion = if visited.contains(&n.sym.id) { " (↻)" } else { "" };
                lines.push(format!(
                    "{indent}{arrow} {} {} {site}{rel} [{}: {}]{caps}{recursion}",
                    display(&n),
                    n.coords(),
                    e.confidence.as_str(),
                    short_evidence(&e, 70)
                ));
                if visited.insert(n.sym.id.clone()) {
                    tree(conn, &n.sym.id, callees, depth + 1, max_depth, rels, visited, lines, with_caps)?;
                }
            }
            None => {
                if callees {
                    lines.push(format!(
                        "{indent}{arrow} ? {} @{} [unresolved: {}]",
                        e.unresolved_symbol.clone().unwrap_or_default(),
                        e.line,
                        short_evidence(&e, 80)
                    ));
                }
            }
        }
    }
    Ok(())
}

fn reachable_count(conn: &Connection, id: &str, callees: bool, depth: usize) -> i64 {
    let sql = if callees {
        "WITH RECURSIVE r(id, d) AS (SELECT ?1, 0 UNION SELECT e.target_id, r.d + 1 FROM r JOIN edge e ON e.source_id = r.id \
         WHERE r.d < ?2 AND e.relation IN ('CALLS','HTTP_CALLS','RESOLVES_DI') AND e.target_id IS NOT NULL) SELECT COUNT(DISTINCT id) - 1 FROM r"
    } else {
        "WITH RECURSIVE r(id, d) AS (SELECT ?1, 0 UNION SELECT e.source_id, r.d + 1 FROM r JOIN edge e ON e.target_id = r.id \
         WHERE r.d < ?2 AND e.relation IN ('CALLS','HTTP_CALLS','RESOLVES_DI')) SELECT COUNT(DISTINCT id) - 1 FROM r"
    };
    conn.query_row(sql, params![id, depth as i64], |r| r.get(0)).unwrap_or(0)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HierarchyParams {
    /// Symbol id or name.
    pub symbol_id: String,
    /// "callers", "callees" (default) or "both".
    pub direction: Option<String>,
    /// Tree depth (default 2, max 6).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_call_hierarchy(ctx: &Ctx, p: HierarchyParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    let depth = p.depth.unwrap_or(2).clamp(1, 6);
    let dir = p.direction.as_deref().unwrap_or("callees");
    let dirs: Vec<bool> = match dir {
        "callers" => vec![false],
        "callees" => vec![true],
        "both" => vec![false, true],
        other => return Err(ToolError::BadArgs(format!("direction must be callers|callees|both, got '{other}'"))),
    };
    let mut out = Out::new();
    out.h(node_line(&n));
    let seeds = expand_type(ctx.conn, &n)?;
    for callees in dirs {
        let label = if callees { "callees" } else { "callers" };
        let total: i64 = seeds.iter().map(|s| reachable_count(ctx.conn, s, callees, depth)).sum();
        out.item(format!("{label} (depth {depth}, {total} reachable):"));
        for s in &seeds {
            let mut lines = Vec::new();
            let mut visited: HashSet<String> = HashSet::from([s.clone()]);
            tree(ctx.conn, s, callees, 0, depth, FLOW_RELATIONS, &mut visited, &mut lines, false)?;
            if seeds.len() > 1 && !lines.is_empty() {
                if let Some(m) = query::node(ctx.conn, s)? {
                    out.item(format!(" {}:", display(&m)));
                }
            }
            for l in lines {
                out.item(l);
            }
        }
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeHierarchyParams {
    /// Type (or member — its declaring type is used).
    pub symbol_id: String,
    /// "supertypes", "subtypes" or "both" (default).
    pub direction: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_type_hierarchy(ctx: &Ctx, p: TypeHierarchyParams) -> ToolResult {
    let mut n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    if !n.sym.kind.is_type() {
        if let Some(parent) = n.sym.parent_id.clone().and_then(|pid| query::node(ctx.conn, &pid).ok().flatten()) {
            n = parent;
        }
    }
    let dir = p.direction.as_deref().unwrap_or("both");
    let mut out = Out::new();
    out.h(format!("{} | {}", node_line(&n), n.sym.signature));
    fn walk(conn: &Connection, id: &str, up: bool, depth: usize, visited: &mut HashSet<String>, out: &mut Out) -> anyhow::Result<()> {
        if depth > 8 {
            return Ok(());
        }
        let edges = if up { query::edges_out(conn, id, &["INHERITS"])? } else { query::edges_in(conn, id, &["INHERITS"])? };
        let ids: Vec<String> = edges.iter().filter_map(|e| if up { e.target_id.clone() } else { Some(e.source_id.clone()) }).collect();
        let nodes = query::nodes_by_ids(conn, &ids)?;
        let indent = "  ".repeat(depth + 1);
        for e in edges {
            let kind = e.metadata.as_ref().and_then(|m| m.get("kind")).and_then(|k| k.as_str()).unwrap_or("extends").to_string();
            let other = if up { e.target_id.clone() } else { Some(e.source_id.clone()) };
            match other.and_then(|o| nodes.get(&o).cloned()) {
                Some(t) => {
                    out.item(format!(
                        "{indent}{} {} [{}]",
                        if up {
                            kind.clone()
                        } else if kind == "implements" {
                            "implemented by".to_string()
                        } else {
                            "extended by".to_string()
                        },
                        node_line(&t),
                        e.confidence.as_str()
                    ));
                    if visited.insert(t.sym.id.clone()) {
                        walk(conn, &t.sym.id, up, depth + 1, visited, out)?;
                    }
                }
                None => {
                    out.item(format!("{indent}{kind} {} (external)", e.unresolved_symbol.clone().unwrap_or_default()));
                }
            }
        }
        Ok(())
    }
    if dir == "supertypes" || dir == "both" {
        out.item("supertypes:");
        walk(ctx.conn, &n.sym.id, true, 0, &mut HashSet::new(), &mut out)?;
    }
    if dir == "subtypes" || dir == "both" {
        out.item("subtypes:");
        walk(ctx.conn, &n.sym.id, false, 0, &mut HashSet::new(), &mut out)?;
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImpactParams {
    /// Symbol id or name.
    pub symbol_id: String,
    /// Transitive depth (default 3, max 8).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_impact_radius(ctx: &Ctx, p: ImpactParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    let depth = p.depth.unwrap_or(3).clamp(1, 8);
    let sql = "WITH RECURSIVE seeds(id) AS (SELECT ?1 UNION SELECT id FROM node WHERE parent_id = ?1), \
               r(id, d) AS (SELECT id, 0 FROM seeds UNION SELECT e.source_id, r.d + 1 FROM r JOIN edge e ON e.target_id = r.id \
               WHERE r.d < ?2 AND e.relation IN ('CALLS','HTTP_CALLS','INHERITS','IMPORTS','READS_VAR','WRITES_VAR','RESOLVES_DI','TESTS')) \
               SELECT id, MIN(d) FROM r WHERE id NOT IN (SELECT id FROM seeds) GROUP BY id ORDER BY 2";
    let mut st = ctx.conn.prepare(sql)?;
    let rows: Vec<(String, i64)> = st.query_map(params![n.sym.id, depth as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    let nodes = nodes_map(ctx.conn, rows.iter().map(|r| r.0.clone()))?;
    let mut by_depth: BTreeMap<i64, usize> = BTreeMap::new();
    let mut files = HashSet::new();
    let mut tests = Vec::new();
    let mut entries = Vec::new();
    let mut items = Vec::new();
    for (id, d) in &rows {
        let Some(m) = nodes.get(id) else { continue };
        *by_depth.entry(*d).or_default() += 1;
        files.insert(m.file_path.clone());
        if m.sym.is_test {
            tests.push(m.clone());
        } else if m.sym.is_entrypoint {
            entries.push(m.clone());
        }
        items.push(format!("d{d} {}", node_line(m)));
    }
    let potential: i64 = ctx
        .conn
        .query_row("SELECT COUNT(*) FROM edge WHERE target_id IS NULL AND metadata LIKE ?1", [format!("%{}%", n.sym.id)], |r| r.get(0))
        .unwrap_or(0);
    let mut out = Out::new();
    out.h(format!("Blast radius of {} {}", display(&n), n.coords()));
    out.h(format!(
        "{} dependent(s) in {} file(s) within depth {depth} [{}] | entrypoints: {} | tests: {}{}",
        rows.len(),
        files.len(),
        by_depth.iter().map(|(d, c)| format!("d{d}:{c}")).collect::<Vec<_>>().join(" "),
        entries.len(),
        tests.len(),
        if potential > 0 { format!(" | +{potential} ambiguous dynamic call(s)") } else { String::new() }
    ));
    for e in &entries {
        out.item(format!("ENTRY {} <{}>", node_line(e), e.sym.entry_kind.clone().unwrap_or_default()));
    }
    for t in &tests {
        out.item(format!("TEST {}", node_line(t)));
    }
    for i in items {
        out.item(i);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Fullstack & DI
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FullstackParams {
    /// Route ("GET /api/orders", "/api/orders/{id}") or a frontend/backend symbol.
    pub api_route_or_call: String,
    /// Depth of the backend call tree (default 3).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

struct ApiRoute {
    handler_id: String,
    method: String,
    path: String,
    normalized: String,
    framework: String,
}

fn api_routes(conn: &Connection) -> anyhow::Result<Vec<ApiRoute>> {
    let mut st =
        conn.prepare("SELECT handler_id, http_method, path, normalized, framework FROM api_route ORDER BY file_path, line_number, id")?;
    let rows = st.query_map([], |r| {
        Ok(ApiRoute {
            handler_id: r.get(0)?,
            method: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            path: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            normalized: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            framework: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        })
    })?;
    Ok(rows.flatten().collect())
}

pub fn trace_fullstack_flow(ctx: &Ctx, p: FullstackParams) -> ToolResult {
    let input = p.api_route_or_call.trim();
    let depth = p.depth.unwrap_or(3).clamp(1, 6);
    let routes = api_routes(ctx.conn)?;
    let mut handlers: Vec<(String, String)> = Vec::new(); // (handler id, route label)
    let mut frontend_only: Vec<EdgeRow> = Vec::new();
    let looks_like_route = input.contains('/');
    if looks_like_route {
        let (method, path) = match input.split_once(' ') {
            Some((m, p)) if m.chars().all(|c| c.is_ascii_alphabetic()) => (Some(m.to_ascii_uppercase()), p.trim()),
            _ => (None, input),
        };
        let norm = normalize_route(path);
        let needle = if norm.starts_with("{}") {
            format!("/{}", norm.split('/').skip_while(|s| *s == "{}").collect::<Vec<_>>().join("/"))
        } else {
            norm.clone()
        };
        let mut matched: Vec<&ApiRoute> = routes.iter().filter(|r| r.normalized == needle).collect();
        if matched.is_empty() {
            matched = routes.iter().filter(|r| r.normalized.ends_with(&needle)).collect();
        }
        if let Some(m) = &method {
            matched.retain(|r| &r.method == m || r.method == "ANY");
        }
        let mut seen = HashSet::new();
        for r in matched {
            if seen.insert(r.handler_id.clone()) {
                handlers.push((r.handler_id.clone(), format!("{} {} [{}]", r.method, r.path, r.framework)));
            }
        }
    } else {
        let n = resolve_symbol(ctx.conn, input)?;
        let route_of = |id: &str| routes.iter().find(|r| r.handler_id == id).map(|r| format!("{} {} [{}]", r.method, r.path, r.framework));
        if let Some(label) = route_of(&n.sym.id) {
            handlers.push((n.sym.id.clone(), label));
        }
        // Frontend side: HTTP_CALLS from the symbol or from its callees.
        let mut frontier = vec![n.sym.id.clone()];
        let mut visited = HashSet::new();
        for _ in 0..3 {
            let mut next = Vec::new();
            for id in frontier.drain(..) {
                if !visited.insert(id.clone()) {
                    continue;
                }
                for e in query::edges_out(ctx.conn, &id, &["HTTP_CALLS", "CALLS"])? {
                    if e.relation == "HTTP_CALLS" {
                        match &e.target_id {
                            Some(t) => {
                                let label = route_of(t).unwrap_or_else(|| "route".into());
                                if !handlers.iter().any(|h| &h.0 == t) {
                                    handlers.push((t.clone(), label));
                                }
                            }
                            None => frontend_only.push(e),
                        }
                    } else if let Some(t) = e.target_id {
                        next.push(t);
                    }
                }
            }
            frontier = next;
        }
    }
    let mut out = Out::new();
    if handlers.is_empty() && frontend_only.is_empty() {
        out.h(format!("No backend route or HTTP call matches '{input}'"));
        // Show unlinked frontend calls with a similar URL to help the user.
        let mut st =
            ctx.conn.prepare("SELECT file_path, http_method, url, line_number FROM http_call ORDER BY file_path, line_number, id")?;
        let calls: Vec<(String, String, String, u32)> = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get(3)?,
                ))
            })?
            .flatten()
            .collect();
        let n = normalize_route(input.split_once(' ').map(|x| x.1).unwrap_or(input));
        for (f, m, u, l) in
            calls.into_iter().filter(|c| normalize_route(&c.2).ends_with(n.trim_start_matches('{').trim_start_matches('}'))).take(10)
        {
            out.item(format!("UNLINKED {m} {u} at {f}:{l}"));
        }
        return Ok(out);
    }
    out.h(format!("Fullstack flow for '{input}': {} backend handler(s)", handlers.len()));
    for (hid, label) in &handlers {
        let Some(h) = query::node(ctx.conn, hid)? else { continue };
        out.item(format!("ROUTE {label} → {}", node_line(&h)));
        let incoming = query::edges_in(ctx.conn, hid, &["HTTP_CALLS"])?;
        let srcs = nodes_map(ctx.conn, incoming.iter().map(|e| e.source_id.clone()).chain(std::iter::once(hid.clone())))?;
        if incoming.is_empty() {
            out.item("  frontend callers: (none linked)");
        } else {
            out.item("  frontend callers:");
        }
        for e in &incoming {
            out.item(format!("    {}", edge_line(e, &srcs)));
            for up in query::edges_in(ctx.conn, &e.source_id, &["CALLS"])?.into_iter().take(4) {
                if let Some(c) = query::node(ctx.conn, &up.source_id)? {
                    out.item(format!("      ↑ {} {}:{} [{}]", display(&c), up.source_file, up.line, up.confidence.as_str()));
                }
            }
        }
        out.item("  backend flow:");
        let mut lines = Vec::new();
        let mut visited = HashSet::from([hid.clone()]);
        tree(ctx.conn, hid, true, 1, depth + 1, &["CALLS", "RESOLVES_DI"], &mut visited, &mut lines, true)?;
        for l in &lines {
            out.item(l.clone());
        }
        // Side effects of everything reached.
        let mut ids: Vec<String> = visited.into_iter().collect();
        ids.push(hid.clone());
        let ph = vec!["?"; ids.len()].join(",");
        let mut st = ctx.conn.prepare(&format!("SELECT DISTINCT mutation_type, target_state FROM state_mutation WHERE method_node_id IN ({ph}) AND mutation_type IN ('database', 'cache', 'global_var')"))?;
        let effects: Vec<String> = st
            .query_map(rusqlite::params_from_iter(ids.iter()), |r| Ok(format!("{} {}", r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .flatten()
            .collect();
        if !effects.is_empty() {
            out.item(format!("  side effects: {}", effects.join("; ")));
        }
    }
    if !frontend_only.is_empty() {
        let srcs = nodes_map(ctx.conn, frontend_only.iter().map(|e| e.source_id.clone()))?;
        for e in &frontend_only {
            out.item(format!("AMBIGUOUS {}", edge_line(e, &srcs)));
        }
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiParams {
    /// Interface / service type name or injection token (e.g. "IOrderService", "PAYMENT_GATEWAY").
    pub interface_or_token: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn resolve_dependency_injection(ctx: &Ctx, p: DiParams) -> ToolResult {
    let raw = p.interface_or_token.trim().trim_matches(|c| c == '"' || c == '\'');
    let key = clean_type(raw);
    let mut st = ctx.conn.prepare(
        "SELECT file_path, service, implementation, lifetime, framework, line_number FROM di_registration WHERE service = ?1 OR service = ?2 ORDER BY file_path, line_number",
    )?;
    let regs: Vec<(String, String, Option<String>, String, String, u32)> = st
        .query_map(params![raw, key], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                r.get(5)?,
            ))
        })?
        .flatten()
        .collect();
    let mut out = Out::new();
    let service_node = query::find_nodes(ctx.conn, &key, 5)?.into_iter().find(|n| n.sym.kind.is_type());
    if regs.is_empty() {
        out.h(format!("No DI container registration found for '{raw}'"));
        if let Some(s) = &service_node {
            let subs = query::edges_in(ctx.conn, &s.sym.id, &["INHERITS"])?;
            let nodes = nodes_map(ctx.conn, subs.iter().map(|e| e.source_id.clone()))?;
            out.h(format!("{} implementation(s) of {} found by type hierarchy:", subs.len(), display(s)));
            for e in subs {
                if let Some(n) = nodes.get(&e.source_id) {
                    out.item(format!("IMPL {} [{}]", node_line(n), e.confidence.as_str()));
                }
            }
        }
    } else {
        out.h(format!("{} registration(s) for '{raw}'", regs.len()));
        for (file, service, imp, lifetime, fw, line) in &regs {
            let imp_name = imp.clone().unwrap_or_else(|| "(factory/value)".into());
            let imp_node = imp
                .as_ref()
                .and_then(|i| query::find_nodes(ctx.conn, &clean_type(i), 5).ok())
                .and_then(|v| v.into_iter().find(|n| n.sym.kind.is_type() || n.sym.kind.is_callable()));
            let at = imp_node.as_ref().map(|n| format!(" {}", n.coords())).unwrap_or_default();
            out.item(format!("{service} → {imp_name}{at} [{lifetime}, {fw}] registered at {file}:{line}"));
        }
    }
    // Consumers: fields / properties / constructor parameters typed with the service.
    let mut st = ctx.conn.prepare(&format!(
        "SELECT {NODE_COLS} FROM node WHERE (type_ref = ?1 OR type_ref LIKE ?2 OR params LIKE ?3) AND derived = 0 ORDER BY file_path, start_line LIMIT 50"
    ))?;
    let consumers: Vec<Node> =
        st.query_map(params![key, format!("{key}<%"), format!("%\"ty\":\"{key}\"%")], node_from_row)?.flatten().collect();
    for c in consumers {
        let how = if c.sym.kind.is_callable() { "as parameter of" } else { "into" };
        out.item(format!("INJECTED {how} {}", node_line(&c)));
    }
    // Inject tokens in signatures (NestJS @Inject('TOKEN')).
    if raw != key || raw.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        let mut st = ctx
            .conn
            .prepare(&format!("SELECT {NODE_COLS} FROM node WHERE signature LIKE ?1 ORDER BY file_path, start_line, id LIMIT 20"))?;
        for c in st.query_map([format!("%@Inject(%{raw}%")], node_from_row)?.flatten() {
            out.item(format!("INJECTED via @Inject('{raw}') in {}", node_line(&c)));
        }
    }
    let _ = json!({});
    Ok(out)
}
