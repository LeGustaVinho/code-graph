//! 5.4 Behavior: state mutations, error flow, capabilities, entrypoints, dead code.

use std::collections::{BTreeMap, HashMap, HashSet};

use codegraph_core::SymbolKind;
use codegraph_graph::query;
use codegraph_graph::store::{edge_from_row, node_from_row, EdgeRow, EDGE_COLS, NODE_COLS};
use codegraph_graph::Node;
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, node_line};
use super::{nodes_map, resolve_symbol, wants_json, Ctx, Out, ToolError, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MutationParams {
    /// Function / method (or type: all of its members).
    pub symbol_id: String,
    /// Also include mutations performed by transitive callees.
    pub include_callees: Option<bool>,
    /// Callee depth when include_callees is set (default 3).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_state_mutations(ctx: &Ctx, p: MutationParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    let depth = if p.include_callees.unwrap_or(false) { p.depth.unwrap_or(3).clamp(1, 8) } else { 0 };
    let sql = "WITH RECURSIVE seeds(id) AS (SELECT ?1 UNION SELECT id FROM node WHERE parent_id = ?1), \
               r(id, d) AS (SELECT id, 0 FROM seeds UNION SELECT e.target_id, r.d + 1 FROM r JOIN edge e ON e.source_id = r.id \
               WHERE r.d < ?2 AND e.relation IN ('CALLS', 'RESOLVES_DI') AND e.target_id IS NOT NULL AND e.confidence <> 'unresolved') \
               SELECT m.method_node_id, m.target_state, m.mutation_type, m.line_number, m.file_path, MIN(r.d) \
               FROM r JOIN state_mutation m ON m.method_node_id = r.id \
               GROUP BY m.method_node_id, m.target_state, m.mutation_type, m.line_number ORDER BY MIN(r.d), m.mutation_type, m.file_path, m.line_number";
    let mut st = ctx.conn.prepare(sql)?;
    let rows: Vec<(String, String, String, u32, String, i64)> = st
        .query_map(params![n.sym.id, depth as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .flatten()
        .collect();
    let nodes = nodes_map(ctx.conn, rows.iter().map(|r| r.0.clone()))?;
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    for r in &rows {
        *by_type.entry(r.2.clone()).or_default() += 1;
    }
    let mut out = Out::new();
    out.h(format!(
        "{} mutation(s) by {}{}: {}",
        rows.len(),
        display(&n),
        if depth > 0 { format!(" incl. callees (depth {depth})") } else { String::new() },
        by_type.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")
    ));
    for (mid, target, mtype, line, file, d) in rows {
        let who = nodes.get(&mid).map(display).unwrap_or_default();
        let via = if d > 0 {
            format!(" (via {who}, depth {d})")
        } else if mid != n.sym.id {
            format!(" (in {who})")
        } else {
            String::new()
        };
        out.item(format!("{mtype} {target} @{file}:{line}{via}"));
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ErrorFlowParams {
    /// Exception type (e.g. "OrderNotFoundException") or a fragment of the error message.
    pub exception_name_or_message: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn trace_error_flow(ctx: &Ctx, p: ErrorFlowParams) -> ToolResult {
    let q = p.exception_name_or_message.trim();
    let mut st = ctx.conn.prepare(&format!(
        "SELECT {} FROM edge e LEFT JOIN node n ON n.id = e.target_id WHERE e.relation = 'THROWS' AND \
         (n.name = ?1 OR n.qualified_name = ?1 OR e.unresolved_symbol = ?1 OR e.metadata LIKE ?2) ORDER BY e.source_file, e.line_number LIMIT 200",
        EDGE_COLS.split(", ").map(|c| format!("e.{c}")).collect::<Vec<_>>().join(", ")
    ))?;
    let throws: Vec<EdgeRow> = st.query_map(params![q, format!("%\"message\":\"%{q}%")], edge_from_row)?.flatten().collect();
    let nodes = nodes_map(ctx.conn, throws.iter().flat_map(|e| std::iter::once(e.source_id.clone()).chain(e.target_id.clone())))?;
    let mut out = Out::new();
    out.h(format!("Error flow for '{q}': {} throw site(s)", throws.len()));
    let mut exceptions: HashSet<String> = HashSet::new();
    for t in &throws {
        let exc =
            t.target_id.as_ref().and_then(|x| nodes.get(x)).map(|n| n.sym.name.clone()).or(t.unresolved_symbol.clone()).unwrap_or_default();
        exceptions.insert(exc.clone());
        let msg =
            t.metadata.as_ref().and_then(|m| m.get("message")).and_then(|m| m.as_str()).map(|m| format!(" \"{m}\"")).unwrap_or_default();
        let thrower = nodes.get(&t.source_id).map(display).unwrap_or_default();
        let mut item = format!("THROW {exc} at {}:{} in {thrower}{msg}", t.source_file, t.line);
        let caught: Vec<EdgeRow> = query::edges_out(ctx.conn, &t.source_id, &["CAUGHT_BY"])?
            .into_iter()
            .filter(|c| c.metadata.as_ref().and_then(|m| m.get("exception")).and_then(|e| e.as_str()) == Some(exc.as_str()))
            .collect();
        let cnodes = nodes_map(ctx.conn, caught.iter().filter_map(|c| c.target_id.clone()))?;
        if caught.is_empty() {
            // Uncaught: which entrypoints does it escape through?
            let sql = "WITH RECURSIVE up(id, d) AS (SELECT ?1, 0 UNION SELECT e.source_id, up.d + 1 FROM up JOIN edge e ON e.target_id = up.id \
                       WHERE up.d < 8 AND e.relation IN ('CALLS', 'HTTP_CALLS')) SELECT DISTINCT n.id FROM up JOIN node n ON n.id = up.id WHERE n.is_entrypoint = 1";
            let mut st = ctx.conn.prepare(sql)?;
            let eps: Vec<String> = st.query_map([&t.source_id], |r| r.get(0))?.flatten().collect();
            let epn = nodes_map(ctx.conn, eps.clone())?;
            if eps.is_empty() {
                item.push_str("\n  UNCAUGHT (no catch found in callers)");
            } else {
                for e in eps {
                    if let Some(n) = epn.get(&e) {
                        item.push_str(&format!(
                            "\n  UNCAUGHT escapes through entrypoint {} <{}>",
                            node_line(n),
                            n.sym.entry_kind.clone().unwrap_or_default()
                        ));
                    }
                }
            }
        }
        for c in &caught {
            let catcher = c.target_id.as_ref().and_then(|x| cnodes.get(x)).map(node_line).unwrap_or_default();
            item.push_str(&format!("\n  CAUGHT by {catcher} [{}: {}]", c.confidence.as_str(), c.evidence));
        }
        out.item(item);
    }
    // Catch sites declared for the exception type(s).
    let mut names: Vec<String> = exceptions.into_iter().collect();
    names.push(q.to_string());
    names.sort();
    names.dedup();
    let ph = (1..=names.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let mut st = ctx.conn.prepare(&format!("SELECT file_path, enclosing_node_id, exception, line_number FROM catch_site WHERE exception IN ({ph}) ORDER BY file_path, line_number"))?;
    let catches: Vec<(String, String, String, u32)> =
        st.query_map(rusqlite::params_from_iter(names.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.flatten().collect();
    let cn = nodes_map(ctx.conn, catches.iter().map(|c| c.1.clone()))?;
    for (f, enc, exc, l) in catches {
        out.item(format!("CATCH ({exc}) at {f}:{l} in {}", cn.get(&enc).map(display).unwrap_or_default()));
    }
    let mut st = ctx.conn.prepare(&format!(
        "SELECT file_path, handler, exception, framework, line_number FROM global_handler WHERE exception IN ({ph}) OR exception = '*' ORDER BY file_path, line_number, id"
    ))?;
    let handlers: Vec<(String, String, String, String, u32)> = st
        .query_map(rusqlite::params_from_iter(names.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .flatten()
        .collect();
    let hn = nodes_map(ctx.conn, handlers.iter().map(|h| h.1.clone()))?;
    // Only handlers of the same runtime as the throw sites are relevant.
    let family = |f: &str| codegraph_core::Language::from_path(std::path::Path::new(f)).map(|l| l.family()).unwrap_or("");
    let fams: HashSet<&str> = throws.iter().map(|t| family(&t.source_file)).collect();
    for (f, h, exc, fw, l) in handlers {
        if !fams.is_empty() && !fams.contains(family(&f)) {
            continue;
        }
        out.item(format!("GLOBAL HANDLER [{fw}] for {exc} at {f}:{l} ({})", hn.get(&h).map(display).unwrap_or_default()));
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityParams {
    /// Tags: DB_READ, DB_WRITE, NETWORK_IO, FILE_IO, LOCAL_STORAGE, ASYNC, AUTH_REQUIRED, CACHE, CONFIG_READ, THROWS, GLOBAL_STATE, LOGGING, EVENT_EMIT.
    pub tags: Vec<String>,
    /// Optional module filter (path fragment or qualified-name prefix).
    pub module: Option<String>,
    /// Match any tag instead of all tags.
    pub match_any: Option<bool>,
    /// Maximum results (default 50).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn find_by_capability(ctx: &Ctx, p: CapabilityParams) -> ToolResult {
    if p.tags.is_empty() {
        return Err(ToolError::BadArgs("tags must not be empty".into()));
    }
    let tags: Vec<String> = p.tags.iter().map(|t| t.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_uppercase()).collect();
    let joiner = if p.match_any.unwrap_or(false) { " OR " } else { " AND " };
    let conds: Vec<String> = (0..tags.len()).map(|i| format!("capabilities LIKE ?{}", i + 1)).collect();
    let mut args: Vec<String> = tags.iter().map(|t| format!("%\"{t}\"%")).collect();
    let mut sql = format!("SELECT {NODE_COLS} FROM node WHERE derived = 0 AND kind <> 'module' AND ({})", conds.join(joiner));
    if let Some(m) = &p.module {
        args.push(format!("%{m}%"));
        args.push(format!("{m}%"));
        sql.push_str(&format!(" AND (file_path LIKE ?{} OR qualified_name LIKE ?{})", args.len() - 1, args.len()));
    }
    sql.push_str(" ORDER BY file_path, start_line");
    let mut st = ctx.conn.prepare(&sql)?;
    let rows: Vec<Node> = st.query_map(rusqlite::params_from_iter(args.iter()), node_from_row)?.flatten().collect();
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    let mut out = Out::new();
    out.h(format!("{} symbol(s) with [{}]", rows.len(), tags.join(joiner.trim())));
    let json = wants_json(&p.format);
    for n in rows.iter().take(limit) {
        let caps: Vec<String> = n.sym.capabilities.iter().cloned().collect();
        if json {
            out.item(json!({ "id": n.sym.id, "name": display(n), "coords": n.coords(), "capabilities": caps }).to_string());
        } else {
            out.item(format!("{} [{}]", node_line(n), caps.join(",")));
        }
    }
    out.unshown = rows.len().saturating_sub(limit);
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntrypointParams {
    /// Filter by entry kind fragment: "route", "main", "cli", "event", "server", "function".
    pub kind: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_entrypoints(ctx: &Ctx, p: EntrypointParams) -> ToolResult {
    let mut st =
        ctx.conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE is_entrypoint = 1 ORDER BY entry_kind, file_path, start_line"))?;
    let mut rows: Vec<Node> = st.query_map([], node_from_row)?.flatten().collect();
    if let Some(k) = &p.kind {
        rows.retain(|n| n.sym.entry_kind.as_deref().unwrap_or("").contains(k.as_str()));
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for n in &rows {
        let cat = n.sym.entry_kind.as_deref().unwrap_or("entry").split_whitespace().next().unwrap_or("entry").to_string();
        *counts.entry(cat).or_default() += 1;
    }
    let mut out = Out::new();
    out.h(format!("{} entrypoint(s): {}", rows.len(), counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")));
    let json = wants_json(&p.format);
    for n in &rows {
        let ek = n.sym.entry_kind.clone().unwrap_or_default();
        if json {
            out.item(json!({ "kind": ek, "id": n.sym.id, "name": display(n), "coords": n.coords() }).to_string());
        } else {
            out.item(format!("{ek} → {} {}", display(n), n.coords()));
        }
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrphanParams {
    /// Kinds to check (default: function, method, class).
    pub kinds: Option<Vec<String>>,
    /// Optional module filter (path fragment).
    pub module: Option<String>,
    /// Include exported / public symbols (default true).
    pub include_exported: Option<bool>,
    /// Include third-party (vendor) code such as Assets/Plugins (default false).
    pub include_vendor: Option<bool>,
    /// Maximum results (default 50).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

const FRAMEWORK_NAMES: &[&str] = &[
    "constructor",
    "__init__",
    "__str__",
    "__repr__",
    "__eq__",
    "__hash__",
    "__call__",
    "__enter__",
    "__exit__",
    "Main",
    "render",
    "componentDidMount",
    "componentWillUnmount",
    "ngOnInit",
    "ngOnDestroy",
    "Dispose",
    "DisposeAsync",
    "ToString",
    "Equals",
    "GetHashCode",
    "OnModelCreating",
    "Configure",
    "ConfigureServices",
    "setUp",
    "tearDown",
    "toString",
    "toJSON",
    "default",
    "handler",
    "middleware",
    "OnGet",
    "OnPost",
];

pub fn find_orphaned_symbols(ctx: &Ctx, p: OrphanParams) -> ToolResult {
    let kinds = p.kinds.clone().unwrap_or_else(|| vec!["function".into(), "method".into(), "class".into()]);
    let conn = ctx.conn;
    // Everything referenced by a resolved edge.
    let mut referenced: HashSet<String> = HashSet::new();
    {
        let mut st = conn.prepare("SELECT DISTINCT target_id FROM edge WHERE target_id IS NOT NULL")?;
        referenced.extend(st.query_map([], |r| r.get::<_, String>(0))?.flatten());
    }
    // Candidates of ambiguous (unresolved) calls and names used dynamically.
    let mut dynamic_ids: HashSet<String> = HashSet::new();
    let mut dynamic_names: HashSet<String> = HashSet::new();
    {
        let mut st = conn.prepare("SELECT unresolved_symbol, metadata FROM edge WHERE target_id IS NULL")?;
        for (sym, meta) in st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))?.flatten() {
            if let Some(s) = sym {
                dynamic_names.insert(s.rsplit('.').next().unwrap_or(&s).to_string());
                dynamic_names.insert(s);
            }
            if let Some(ids) = meta
                .as_deref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                .and_then(|v| v.get("candidate_ids").cloned())
            {
                if let Some(arr) = ids.as_array() {
                    dynamic_ids.extend(arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())));
                }
            }
        }
    }
    // Type structure: children, resolved bases and external bases.
    let mut children: HashMap<String, Vec<(String, String)>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT id, parent_id, name FROM node WHERE parent_id IS NOT NULL AND derived = 0")?;
        for (id, parent, name) in
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.flatten()
        {
            children.entry(parent).or_default().push((id, name));
        }
    }
    let mut bases: HashMap<String, Vec<String>> = HashMap::new();
    let mut external_base: HashSet<String> = HashSet::new();
    // Types deriving directly from a Unity type whose instances live in scenes / assets.
    let mut serialized_root: HashSet<String> = HashSet::new();
    {
        let mut st = conn.prepare("SELECT source_id, target_id, unresolved_symbol FROM edge WHERE relation = 'INHERITS'")?;
        for (src, tgt, ext) in st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?
            .flatten()
        {
            match tgt {
                Some(t) => bases.entry(src).or_default().push(t),
                None => {
                    let name = ext.unwrap_or_default();
                    let short = name.rsplit('.').next().unwrap_or(&name).to_string();
                    let lineage = codegraph_core::unity::lineage(&short);
                    if codegraph_core::unity::SERIALIZED_ROOTS.iter().any(|r| *r == short || lineage.contains(r)) {
                        serialized_root.insert(src.clone());
                    }
                    external_base.insert(src);
                }
            }
        }
    }
    // Engine / framework entry points: a type declaring one is in use.
    let entry_ids: HashSet<String> = {
        let mut st = conn.prepare("SELECT id FROM node WHERE is_entrypoint = 1")?;
        let rows: HashSet<String> = st.query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
        rows
    };
    // Generated sources (designers, source generators) are never dead code to delete.
    let generated_files: HashSet<String> = {
        let mut st = conn.prepare("SELECT file_path FROM node WHERE kind = 'module' AND attrs LIKE '%\"generated\"%'")?;
        let rows: HashSet<String> = st.query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
        rows
    };
    // Third-party code is not the project's to clean up.
    let vendor_files: HashSet<String> = if p.include_vendor.unwrap_or(false) {
        HashSet::new()
    } else {
        let mut st = conn.prepare("SELECT file_path FROM file_meta WHERE is_vendor = 1")?;
        let rows: HashSet<String> = st.query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
        rows
    };
    let used = |id: &str| referenced.contains(id) || dynamic_ids.contains(id) || entry_ids.contains(id);
    let used_or_members_used = |id: &str| used(id) || children.get(id).map(|k| k.iter().any(|(c, _)| used(c))).unwrap_or(false);
    let ancestors = |t: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![t.to_string()];
        let mut seen = HashSet::new();
        while let Some(x) = stack.pop() {
            if out.len() > 32 {
                break;
            }
            for b in bases.get(&x).cloned().unwrap_or_default() {
                if seen.insert(b.clone()) {
                    out.push(b.clone());
                    stack.push(b);
                }
            }
        }
        out
    };
    // MonoBehaviours / ScriptableObjects (also through project base classes) are attached in
    // scenes and assets, which are not indexed: the type and its public members (UnityEvent
    // targets, inspector-wired callbacks) are reachable without a code reference.
    let is_serialized = |t: &str| -> bool { serialized_root.contains(t) || ancestors(t).iter().any(|a| serialized_root.contains(a)) };

    let ph = (1..=kinds.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT {NODE_COLS} FROM node WHERE derived = 0 AND kind IN ({ph}) AND is_entrypoint = 0 AND is_test = 0 ORDER BY file_path, start_line"
    );
    let mut st = conn.prepare(&sql)?;
    let rows: Vec<Node> = st.query_map(rusqlite::params_from_iter(kinds.iter()), node_from_row)?.flatten().collect();
    let include_exported = p.include_exported.unwrap_or(true);
    let mut out_rows = Vec::new();
    for n in rows {
        let s = &n.sym;
        if used_or_members_used(&s.id) || generated_files.contains(&n.file_path) || vendor_files.contains(&n.file_path) {
            continue;
        }
        if s.kind.is_type() && is_serialized(&s.id) {
            continue;
        }
        if !s.kind.is_type() && s.visibility != "private" && s.parent_id.as_deref().map(is_serialized).unwrap_or(false) {
            continue;
        }
        if FRAMEWORK_NAMES.contains(&s.name.as_str()) || s.name.starts_with("__") || dynamic_names.contains(&s.name) {
            continue;
        }
        // Decorated / attributed members are invoked by frameworks (`#if …` condition tags
        // added by the preprocessor are not attributes).
        if s.attrs.iter().any(|a| !a.starts_with('#')) && s.kind != SymbolKind::Class {
            continue;
        }
        if s.kind == SymbolKind::Constructor || (s.kind == SymbolKind::Method && s.is_abstract) {
            continue;
        }
        if !include_exported && s.is_exported {
            continue;
        }
        if let Some(m) = &p.module {
            if !n.file_path.contains(m.as_str()) {
                continue;
            }
        }
        // Implementations of a used abstraction are reached polymorphically.
        if s.kind.is_type() && ancestors(&s.id).iter().any(|a| used_or_members_used(a)) {
            continue;
        }
        // Overrides / interface implementations are reached through their base declaration.
        if s.kind == SymbolKind::Method {
            if let Some(parent) = &s.parent_id {
                let overrides =
                    ancestors(parent).iter().any(|a| children.get(a).map(|k| k.iter().any(|(_, name)| name == &s.name)).unwrap_or(false));
                if overrides || (external_base.contains(parent) && s.visibility != "private") {
                    continue;
                }
            }
        }
        out_rows.push(n);
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    let mut out = Out::new();
    out.h(format!("{} orphan candidate(s) (no incoming CALLS/IMPORTS/INHERITS/DI/HTTP/TESTS/READS edges)", out_rows.len()));
    let json = wants_json(&p.format);
    for n in out_rows.iter().take(limit) {
        let vis = if n.sym.is_exported { " [exported]" } else { "" };
        if json {
            out.item(json!({ "id": n.sym.id, "name": display(n), "coords": n.coords(), "exported": n.sym.is_exported }).to_string());
        } else {
            out.item(format!("{}{vis}", node_line(n)));
        }
    }
    out.unshown = out_rows.len().saturating_sub(limit);
    Ok(out)
}
