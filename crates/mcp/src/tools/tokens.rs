//! 5.1 Token economy & aggregation: batch_query, get_file_skeleton,
//! simulate_signature_change, get_flow_diagram.

use std::collections::{HashMap, HashSet};

use codegraph_core::render;
use codegraph_core::{Language, SymbolKind};
use codegraph_graph::query;
use codegraph_graph::store::{edge_from_row, EDGE_COLS};
use codegraph_graph::{EdgeRow, Node};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::fmt::{conf_arrow, display, node_line};
use super::navigation::shortest_path;
use super::{nodes_map, resolve_file_arg, resolve_symbol, run_tool, Ctx, Out, ToolError, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchOp {
    /// Tool name, e.g. "get_file_outline".
    pub tool: String,
    /// Arguments object for that tool.
    #[serde(default)]
    pub args: Option<Value>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchParams {
    /// Up to 20 operations executed in order.
    pub operations: Vec<BatchOp>,
    /// Total token budget, split evenly across operations that set none.
    pub max_token_budget: Option<usize>,
    /// Output format: "compact" (default).
    pub format: Option<String>,
}

pub fn batch_query(ctx: &Ctx, p: BatchParams) -> ToolResult {
    if p.operations.is_empty() || p.operations.len() > 20 {
        return Err(ToolError::BadArgs("operations must contain 1..=20 items".into()));
    }
    let n = p.operations.len();
    let per_op = p.max_token_budget.map(|b| (b / n).max(40));
    let mut out = Out::new();
    for (i, op) in p.operations.into_iter().enumerate() {
        let text = if op.tool == "batch_query" {
            "ERROR: nested batch_query is not allowed".to_string()
        } else {
            let mut args = op.args.unwrap_or(Value::Object(Default::default()));
            if let (Some(b), Some(obj)) = (per_op, args.as_object_mut()) {
                obj.entry("max_token_budget").or_insert(Value::from(b));
            }
            match run_tool(ctx, &op.tool, args) {
                Ok(t) => t,
                Err(e) => format!("ERROR: {e}"),
            }
        };
        out.item(format!("### {}. {}\n{}", i + 1, op.tool, text));
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SkeletonParams {
    /// File path (relative to the project root; suffixes like "OrderService.cs" are accepted).
    pub file_path: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_file_skeleton(ctx: &Ctx, p: SkeletonParams) -> ToolResult {
    let file = resolve_file_arg(ctx.conn, &p.file_path)?;
    let nodes = query::nodes_in_file(ctx.conn, &file)?;
    let lang = Language::from_path(std::path::Path::new(&file)).unwrap_or(Language::TypeScript);
    let lines = query::file_line_count(ctx.conn, &file).unwrap_or(0);
    let mut syms: Vec<codegraph_core::Symbol> = nodes.into_iter().map(|n| n.sym).collect();
    // In ES modules, only exported declarations form the module's public surface.
    // A class's public members are part of that surface even though they do not carry
    // their own export modifier. Other languages (including Python and Rust) have
    // different public API rules, so retain the renderer's native visibility checks.
    let es_module = matches!(lang, Language::TypeScript | Language::Tsx | Language::JavaScript);
    let module = syms.iter().find(|s| s.kind == SymbolKind::Module).map(|s| s.id.clone());
    if es_module {
        if let Some(module) = module {
            let exported: HashSet<String> = syms.iter()
                .filter(|s| s.parent_id.as_deref() == Some(&module) && s.is_exported)
                .map(|s| s.id.clone()).collect();
            if !exported.is_empty() {
                let by_id: HashMap<String, (Option<String>, bool)> = syms.iter()
                    .map(|s| (s.id.clone(), (s.parent_id.clone(), s.kind.is_type() || s.kind == SymbolKind::Impl)))
                    .collect();
                syms.retain(|s| {
                    if s.kind == SymbolKind::Module || exported.contains(&s.id) {
                        return true;
                    }
                    let mut parent = s.parent_id.as_ref();
                    while let Some(id) = parent {
                        if exported.contains(id) { return true; }
                        let Some((next, is_type)) = by_id.get(id) else { break };
                        // Nested local functions are implementation details.
                        if !is_type { break; }
                        parent = next.as_ref();
                    }
                    false
                });
            }
        }
    }
    let rendered = render::skeleton(&file, lines, lang, &syms);
    let mut out = Out::new();
    let mut it = rendered.into_iter();
    if let Some(h) = it.next() {
        out.h(h);
    }
    for l in it {
        out.item(l);
    }
    if out.items.is_empty() {
        out.item("(no public declarations)");
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SimulateParams {
    /// Function / method id or name (e.g. "OrderService.PlaceOrderAsync").
    pub symbol_id: String,
    /// Parameter count after the change.
    pub new_param_count: u32,
    /// Required (non-optional) parameters after the change. Defaults: added params are required, removed params reduce both.
    pub new_required_count: Option<u32>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

/// Same-named members in the supertypes and subtypes of `node`'s declaring type.
fn related_declarations(ctx: &Ctx, node: &Node) -> Result<Vec<Node>, ToolError> {
    let Some(parent) = &node.sym.parent_id else { return Ok(Vec::new()) };
    let mut types: HashSet<String> = HashSet::new();
    let mut stack = vec![parent.clone()];
    while let Some(t) = stack.pop() {
        if !types.insert(t.clone()) || types.len() > 64 {
            continue;
        }
        for e in query::edges_out(ctx.conn, &t, &["INHERITS"])? {
            if let Some(x) = e.target_id {
                stack.push(x);
            }
        }
        for e in query::edges_in(ctx.conn, &t, &["INHERITS"])? {
            stack.push(e.source_id);
        }
    }
    types.remove(parent);
    let mut out = Vec::new();
    for t in types {
        for c in query::children(ctx.conn, &t)? {
            if c.sym.name == node.sym.name && c.sym.kind.is_callable() {
                out.push(c);
            }
        }
    }
    Ok(out)
}

pub fn simulate_signature_change(ctx: &Ctx, p: SimulateParams) -> ToolResult {
    let node = resolve_symbol(ctx.conn, &p.symbol_id)?;
    let Some(params) = node.sym.params.clone() else {
        return Err(ToolError::BadArgs(format!("{} is not a callable with known parameters", display(&node))));
    };
    let (old_total, old_req) = (params.total, params.required);
    let new_total = p.new_param_count;
    let new_req =
        p.new_required_count.unwrap_or(if new_total >= old_total { old_req + (new_total - old_total) } else { old_req.min(new_total) });
    if new_req > new_total {
        return Err(ToolError::BadArgs("new_required_count cannot exceed new_param_count".into()));
    }
    let related = related_declarations(ctx, &node)?;
    let mut targets: Vec<Node> = vec![node.clone()];
    targets.extend(related.iter().cloned());
    let mut breaks = Vec::new();
    let mut ok = Vec::new();
    let mut unknown = Vec::new();
    let mut seen = HashSet::new();
    let mut edges: Vec<EdgeRow> = Vec::new();
    for t in &targets {
        edges.extend(query::edges_in(ctx.conn, &t.sym.id, &["CALLS"])?);
    }
    let nodes = nodes_map(ctx.conn, edges.iter().map(|e| e.source_id.clone()))?;
    for e in &edges {
        if !seen.insert((e.source_id.clone(), e.line)) {
            continue;
        }
        let caller = nodes.get(&e.source_id).map(display).unwrap_or_default();
        let args = e.metadata.as_ref().and_then(|m| m.get("args")).and_then(|a| a.as_u64()).map(|a| a as u32);
        let via = if e.target_id.as_deref() != Some(node.sym.id.as_str()) { " (via related declaration)" } else { "" };
        match args {
            Some(a) => {
                let fails = a < new_req || (!params.variadic && a > new_total);
                let line = format!("{}:{} {caller} passes {a} arg(s) [{}]{via}", e.source_file, e.line, e.confidence.as_str());
                if fails {
                    breaks.push(line);
                } else {
                    ok.push(line);
                }
            }
            None => unknown.push(format!("{}:{} {caller} (argument count unknown) [{}]", e.source_file, e.line, e.confidence.as_str())),
        }
    }
    // Ambiguous dynamic dispatch that may reach this symbol.
    let mut potential = Vec::new();
    {
        let mut st = ctx
            .conn
            .prepare(&format!("SELECT {EDGE_COLS} FROM edge WHERE relation = 'CALLS' AND target_id IS NULL AND metadata LIKE ?1"))?;
        let rows: Vec<EdgeRow> = st.query_map([format!("%{}%", node.sym.id)], edge_from_row)?.flatten().collect();
        let srcs = nodes_map(ctx.conn, rows.iter().map(|e| e.source_id.clone()))?;
        for e in rows {
            let caller = srcs.get(&e.source_id).map(display).unwrap_or_default();
            potential.push(format!(
                "{}:{} {caller} -> {} [unresolved: {}]",
                e.source_file,
                e.line,
                e.unresolved_symbol.clone().unwrap_or_default(),
                e.evidence
            ));
        }
    }
    let mut out = Out::new();
    out.h(format!(
        "Simulating {} {}: {old_total} param(s) ({old_req} required{}) → {new_total} param(s) ({new_req} required)",
        display(&node),
        node.coords(),
        if params.variadic { ", variadic" } else { "" }
    ));
    out.h(format!(
        "{} call site(s) break, {} compatible, {} unknown, {} related declaration(s) to update, {} ambiguous dynamic call(s)",
        breaks.len(),
        ok.len(),
        unknown.len(),
        related.len(),
        potential.len()
    ));
    for b in breaks {
        out.item(format!("BREAKS {b}"));
    }
    for r in &related {
        out.item(format!("UPDATE SIGNATURE {} | {}", node_line(r), r.sym.signature));
    }
    for u in potential {
        out.item(format!("POTENTIAL {u}"));
    }
    for u in unknown {
        out.item(format!("CHECK {u}"));
    }
    for o in ok {
        out.item(format!("OK {o}"));
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowParams {
    /// One symbol (diagram of its neighbourhood) or several (paths between consecutive symbols).
    pub symbols: Vec<String>,
    /// "mermaid" (default) or "text".
    pub format: Option<String>,
    /// Maximum edges in the diagram (default 6).
    pub max_edges: Option<usize>,
    pub max_token_budget: Option<usize>,
}

pub fn get_flow_diagram(ctx: &Ctx, p: FlowParams) -> ToolResult {
    if p.symbols.is_empty() {
        return Err(ToolError::BadArgs("symbols must not be empty".into()));
    }
    let max_edges = p.max_edges.unwrap_or(6).clamp(1, 30);
    let roots: Vec<Node> = p.symbols.iter().map(|s| resolve_symbol(ctx.conn, s)).collect::<Result<_, _>>()?;
    let rels = ["CALLS", "HTTP_CALLS", "RESOLVES_DI"];
    let mut edges: Vec<EdgeRow> = Vec::new();
    if roots.len() == 1 {
        let r = &roots[0];
        let mut seeds = vec![r.sym.id.clone()];
        if r.sym.kind.is_type() {
            seeds.extend(query::children(ctx.conn, &r.sym.id)?.into_iter().filter(|c| c.sym.kind.is_callable()).map(|c| c.sym.id));
        }
        for s in &seeds {
            let mut inc = query::edges_in(ctx.conn, s, &rels)?;
            inc.truncate(2);
            edges.extend(inc);
            edges.extend(query::edges_out(ctx.conn, s, &rels)?);
        }
    } else {
        for w in roots.windows(2) {
            let to: HashSet<String> = std::iter::once(w[1].sym.id.clone()).collect();
            if let Some(path) = shortest_path(ctx.conn, vec![w[0].sym.id.clone()], &to, 8, &rels)?.0 {
                edges.extend(path);
            }
        }
    }
    let mut seen = HashSet::new();
    edges.retain(|e| seen.insert((e.source_id.clone(), e.target_id.clone().or(e.unresolved_symbol.clone()))));
    if roots.len() == 1 {
        // Neighbourhood view: keep the most certain edges when trimming. Paths keep hop order.
        edges.sort_by_key(|e| e.confidence);
    }
    edges.truncate(max_edges);
    let ids: Vec<String> = edges.iter().flat_map(|e| std::iter::once(e.source_id.clone()).chain(e.target_id.clone())).collect();
    let nodes = nodes_map(ctx.conn, ids)?;
    let label = |id: &str| nodes.get(id).map(display).unwrap_or_else(|| id.to_string()).replace('"', "'");
    let mut out = Out::new();
    if edges.is_empty() {
        out.h("(no call edges found for the given symbols)");
        return Ok(out);
    }
    let text_mode = p.format.as_deref() == Some("text");
    if text_mode {
        for e in &edges {
            let t = e.target_id.as_deref().map(label).unwrap_or_else(|| format!("? {}", e.unresolved_symbol.clone().unwrap_or_default()));
            out.item(format!("{} -> {} [{}]", label(&e.source_id), t, e.confidence.as_str()));
        }
        return Ok(out);
    }
    let mut ids_map: HashMap<String, String> = HashMap::new();
    let mut next = 0;
    let mut alias = |k: &str, ids_map: &mut HashMap<String, String>| -> (String, bool) {
        if let Some(a) = ids_map.get(k) {
            return (a.clone(), false);
        }
        let a = format!("n{next}");
        next += 1;
        ids_map.insert(k.to_string(), a.clone());
        (a, true)
    };
    out.h("flowchart LR");
    for e in &edges {
        let (sa, snew) = alias(&e.source_id, &mut ids_map);
        let sdecl = if snew { format!("{sa}[\"{}\"]", label(&e.source_id)) } else { sa };
        let (tk, tl) = match &e.target_id {
            Some(t) => (t.clone(), label(t)),
            None => (
                format!("?{}", e.unresolved_symbol.clone().unwrap_or_default()),
                format!("? {}", e.unresolved_symbol.clone().unwrap_or_default().replace('"', "'")),
            ),
        };
        let (ta, tnew) = alias(&tk, &mut ids_map);
        let tdecl = if tnew { format!("{ta}[\"{tl}\"]") } else { ta };
        let rel = if e.relation == "CALLS" { String::new() } else { format!(" {}", e.relation.to_ascii_lowercase()) };
        out.item(format!("  {sdecl} {}|{}{rel}| {tdecl}", conf_arrow(e.confidence), e.confidence.as_str()));
    }
    let _ = SymbolKind::Module;
    Ok(out)
}
