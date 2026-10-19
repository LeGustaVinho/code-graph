//! 5.2 Discovery: search_universal, find_literal, find_variable_or_const.

use std::time::Instant;

use codegraph_graph::query;
use codegraph_graph::store::{node_from_row, NODE_COLS};
use codegraph_graph::Node;
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, node_line, node_line_sig};
use super::{nodes_map, wants_json, Ctx, Out, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Text to search (substring / trigram match on names, qualified names, signatures and paths).
    pub query: String,
    /// Optional kind filter: class, interface, method, function, variable, const, field, property, data_model, test, enum, module, literal.
    pub kind: Option<String>,
    /// Maximum results (default 20).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
    /// "compact" (default) or "json".
    pub format: Option<String>,
}

fn node_json(n: &Node) -> String {
    json!({
        "id": n.sym.id,
        "kind": n.sym.kind.as_str(),
        "name": display(n),
        "qualified_name": n.sym.qualified_name,
        "coords": n.coords(),
        "signature": n.sym.signature,
    })
    .to_string()
}

pub fn search_universal(ctx: &Ctx, p: SearchParams) -> ToolResult {
    let t0 = Instant::now();
    let limit = p.limit.unwrap_or(20).clamp(1, 200);
    let include_literals = p.kind.as_deref() == Some("literal");
    let (rows, total) = query::search(ctx.conn, &p.query, p.kind.as_deref(), limit, include_literals)?;
    let mut out = Out::new();
    out.h(format!("{} match(es) for '{}' ({:.1} ms)", total, p.query, t0.elapsed().as_secs_f64() * 1000.0));
    let json = wants_json(&p.format);
    for n in &rows {
        out.item(if json { node_json(n) } else { node_line_sig(n) });
    }
    out.unshown = total.saturating_sub(rows.len());
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiteralParams {
    /// Literal text (substring match unless exact=true).
    pub query: String,
    /// Exact value match.
    pub exact: Option<bool>,
    /// Filter by literal type: string, template, number, regex.
    pub literal_type: Option<String>,
    /// Maximum results (default 30).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn find_literal(ctx: &Ctx, p: LiteralParams) -> ToolResult {
    let limit = p.limit.unwrap_or(30).clamp(1, 500) as i64;
    let exact = p.exact.unwrap_or(false);
    let type_clause = if p.literal_type.is_some() { " AND l.literal_type = ?2" } else { " AND ?2 IS NULL" };
    let (sql, arg) = if exact {
        (format!("SELECT l.literal_value, l.literal_type, l.file_path, l.line_number, l.column_number, l.enclosing_node_id FROM literal_occurrence l WHERE l.literal_value = ?1{type_clause} ORDER BY l.file_path, l.line_number LIMIT 1000"), p.query.clone())
    } else if p.query.chars().count() >= 3 {
        (
            format!("SELECT l.literal_value, l.literal_type, l.file_path, l.line_number, l.column_number, l.enclosing_node_id FROM literal_fts f JOIN literal_occurrence l ON l.id = f.rowid WHERE literal_fts MATCH ?1{type_clause} ORDER BY length(l.literal_value), l.file_path, l.line_number LIMIT 1000"),
            format!("\"{}\"", p.query.replace('"', "\"\"")),
        )
    } else {
        (format!("SELECT l.literal_value, l.literal_type, l.file_path, l.line_number, l.column_number, l.enclosing_node_id FROM literal_occurrence l WHERE l.literal_value LIKE ?1{type_clause} ORDER BY l.file_path, l.line_number LIMIT 1000"), format!("%{}%", p.query))
    };
    let mut st = ctx.conn.prepare(&sql)?;
    let rows: Vec<(String, String, String, u32, u32, Option<String>)> = st
        .query_map(params![arg, p.literal_type], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .flatten()
        .collect();
    let total = rows.len();
    let shown: Vec<_> = rows.into_iter().take(limit as usize).collect();
    let nodes = nodes_map(ctx.conn, shown.iter().filter_map(|r| r.5.clone()))?;
    let mut out = Out::new();
    out.h(format!("{total} occurrence(s) of literal '{}'{}", p.query, if exact { " (exact)" } else { "" }));
    let json = wants_json(&p.format);
    for (v, t, f, l, c, enc) in &shown {
        let encl = enc.as_ref().and_then(|e| nodes.get(e)).map(display).unwrap_or_default();
        if json {
            out.item(json!({ "value": v, "type": t, "coords": format!("{f}:{l}:{c}"), "enclosing": encl }).to_string());
        } else {
            out.item(format!("\"{}\" {f}:{l}:{c} in {encl} ({t})", codegraph_core::extract::truncate(v, 120)));
        }
    }
    out.unshown = total.saturating_sub(shown.len());
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VarParams {
    /// Variable / constant / field name (or `Type.field`).
    pub name: String,
    /// Optional scope: file path fragment or qualified-name prefix.
    pub scope: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn find_variable_or_const(ctx: &Ctx, p: VarParams) -> ToolResult {
    let (owner, name) = match p.name.rsplit_once('.') {
        Some((o, n)) => (Some(o.to_string()), n.to_string()),
        None => (None, p.name.clone()),
    };
    let mut st = ctx.conn.prepare(&format!(
        "SELECT {NODE_COLS} FROM node WHERE name = ?1 AND kind IN ('variable', 'const', 'field', 'property') ORDER BY file_path, start_line"
    ))?;
    let mut decls: Vec<Node> = st.query_map([&name], node_from_row)?.flatten().collect();
    if let Some(o) = &owner {
        decls.retain(|n| n.sym.qualified_name.ends_with(&format!("{o}.{name}")));
    }
    if let Some(scope) = &p.scope {
        decls.retain(|n| n.file_path.contains(scope.as_str()) || n.sym.qualified_name.starts_with(scope.as_str()));
    }
    let mut out = Out::new();
    if decls.is_empty() {
        out.h(format!("No variable, constant or field named '{}'", p.name));
        return Ok(out);
    }
    out.h(format!("{} declaration(s) of '{}'", decls.len(), p.name));
    for d in decls.iter().take(20) {
        let mut item = format!("DECL {}", node_line(d));
        if let Some(t) = &d.sym.type_ref {
            item.push_str(&format!(" : {t}"));
        }
        if let Some(v) = &d.sym.value_text {
            item.push_str(&format!(" = {}", codegraph_core::extract::truncate(v, 80)));
        }
        let mut edges = query::edges_in(ctx.conn, &d.sym.id, &["READS_VAR", "WRITES_VAR"])?;
        edges.sort_by(|a, b| a.source_file.cmp(&b.source_file).then(a.line.cmp(&b.line)));
        let srcs = nodes_map(ctx.conn, edges.iter().map(|e| e.source_id.clone()))?;
        let reads = edges.iter().filter(|e| e.relation == "READS_VAR").count();
        let writes = edges.len() - reads;
        item.push_str(&format!(" ({reads} read(s), {writes} write(s))"));
        for e in &edges {
            let who = srcs.get(&e.source_id).map(display).unwrap_or_default();
            let rw = if e.relation == "WRITES_VAR" { "W" } else { "R" };
            item.push_str(&format!("\n  {rw} {}:{} in {who} [{}]", e.source_file, e.line, e.confidence.as_str()));
        }
        out.item(item);
    }
    out.unshown = decls.len().saturating_sub(20);
    Ok(out)
}
