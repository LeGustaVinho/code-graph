//! 5.5 Data contracts, configuration & surgical extraction.

use std::collections::BTreeMap;

use codegraph_core::render;
use codegraph_core::{Language, SymbolKind};
use codegraph_graph::query;
use codegraph_graph::store::{node_from_row, NODE_COLS};
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, node_line};
use super::{nodes_map, resolve_file_arg, resolve_symbol, wants_json, Ctx, Out, ToolError, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelParams {
    /// Model / entity name. Omit to list every data model.
    pub model_name: Option<String>,
    /// "compact" (default), "mermaid" (erDiagram) or "json".
    pub format: Option<String>,
    pub max_token_budget: Option<usize>,
}

struct FieldRow {
    name: String,
    ty: String,
    pk: bool,
    fk: bool,
    target: Option<String>,
    card: Option<String>,
    line: u32,
}

fn fields_of(ctx: &Ctx, model_id: &str) -> rusqlite::Result<Vec<FieldRow>> {
    let mut st = ctx.conn.prepare(
        "SELECT field_name, field_type, is_primary_key, is_foreign_key, target_model, relation_cardinality, line_number FROM data_model_field WHERE model_node_id = ?1 ORDER BY line_number",
    )?;
    let rows = st.query_map([model_id], |r| {
        Ok(FieldRow {
            name: r.get(0)?,
            ty: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            pk: r.get(2)?,
            fk: r.get(3)?,
            target: r.get(4)?,
            card: r.get(5)?,
            line: r.get::<_, Option<u32>>(6)?.unwrap_or(0),
        })
    })?;
    Ok(rows.flatten().collect())
}

pub fn get_data_model(ctx: &Ctx, p: ModelParams) -> ToolResult {
    let mut st = ctx.conn.prepare("SELECT model_node_id, name, framework, table_name FROM data_model ORDER BY name, framework")?;
    let mut models: Vec<(String, String, String, Option<String>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get(3)?)))?
        .flatten()
        .collect();
    if let Some(name) = &p.model_name {
        models.retain(|m| m.1.eq_ignore_ascii_case(name) || m.3.as_deref().map(|t| t.eq_ignore_ascii_case(name)).unwrap_or(false));
        if models.is_empty() {
            return Err(ToolError::NotFound(format!(
                "No data model named '{name}'. Call get_data_model without arguments to list models."
            )));
        }
    }
    let nodes = nodes_map(ctx.conn, models.iter().map(|m| m.0.clone()))?;
    let mut out = Out::new();
    let fmt = p.format.as_deref().unwrap_or("compact");
    if fmt == "mermaid" {
        out.h("erDiagram");
        for (id, name, _, _) in &models {
            let fields = fields_of(ctx, id)?;
            let mut body = format!("  {name} {{");
            for f in fields.iter().filter(|f| f.card.is_none() || f.fk) {
                let ty: String = f.ty.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
                let key = if f.pk {
                    " PK"
                } else if f.fk {
                    " FK"
                } else {
                    ""
                };
                body.push_str(&format!("\n    {} {}{key}", if ty.is_empty() { "any" } else { &ty }, f.name));
            }
            body.push_str("\n  }");
            out.item(body);
            for f in fields.iter().filter(|f| f.card.is_some() && !f.fk) {
                let t = f.target.clone().unwrap_or_default();
                let rel = match f.card.as_deref() {
                    Some("1:N") => "||--o{",
                    Some("N:1") => "}o--||",
                    Some("N:N") => "}o--o{",
                    _ => "||--||",
                };
                out.item(format!("  {name} {rel} {t} : {}", f.name));
            }
        }
        return Ok(out);
    }
    let json = fmt == "json";
    if p.model_name.is_none() {
        out.h(format!("{} data model(s)", models.len()));
    }
    for (id, name, fw, table) in &models {
        let fields = fields_of(ctx, id)?;
        let coords = nodes.get(id).map(|n| n.coords()).unwrap_or_default();
        if json {
            let fj: Vec<_> = fields
                .iter()
                .map(|f| json!({ "name": f.name, "type": f.ty, "pk": f.pk, "fk": f.fk, "target": f.target, "cardinality": f.card, "line": f.line }))
                .collect();
            out.item(json!({ "model": name, "framework": fw, "table": table, "coords": coords, "fields": fj }).to_string());
            continue;
        }
        if p.model_name.is_none() {
            let mut rels: Vec<String> = fields
                .iter()
                .filter(|f| f.card.is_some() && f.target.is_some())
                .map(|f| format!("{}({})", f.target.clone().unwrap_or_default(), f.card.clone().unwrap_or_default()))
                .collect();
            rels.dedup();
            out.item(format!(
                "{name} [{fw}] {coords} fields:{}{}",
                fields.len(),
                if rels.is_empty() { String::new() } else { format!(" rel: {}", rels.join(", ")) }
            ));
            continue;
        }
        let mut item = format!("MODEL {name} [{fw}] {coords}{}", table.as_ref().map(|t| format!(" table={t}")).unwrap_or_default());
        for f in &fields {
            let key = match (f.pk, f.fk) {
                (true, _) => "PK ",
                (false, true) => "FK ",
                _ => "",
            };
            let rel = match (&f.target, &f.card) {
                (Some(t), Some(c)) => format!(" → {t} ({c})"),
                (Some(t), None) => format!(" → {t}"),
                _ => String::new(),
            };
            item.push_str(&format!("\n  {key}{}: {}{rel} :{}", f.name, f.ty, f.line));
        }
        out.item(item);
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigParams {
    /// Optional key filter (substring, case-insensitive).
    pub filter: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_config_matrix(ctx: &Ctx, p: ConfigParams) -> ToolResult {
    let mut st = ctx.conn.prepare("SELECT config_key, source_type, file_path, line_number, default_value, usage, enclosing_node_id FROM config_dependency ORDER BY config_key, usage DESC, file_path, line_number")?;
    let rows: Vec<(String, String, String, u32, Option<String>, String, Option<String>)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.flatten().collect();
    let encl = nodes_map(ctx.conn, rows.iter().filter_map(|r| r.6.clone()))?;
    let mut keys: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (key, src, file, line, default, usage, enc) in &rows {
        if let Some(f) = &p.filter {
            if !key.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) {
                continue;
            }
        }
        let e = keys.entry(key.clone()).or_default();
        if usage == "define" {
            let v = default.as_ref().map(|d| format!("={}", codegraph_core::extract::truncate(d, 40))).unwrap_or_default();
            e.1.push(format!("{file}:{line}{v}"));
        } else {
            let who = enc.as_ref().and_then(|x| encl.get(x)).map(display).unwrap_or_default();
            let d = default.as_ref().map(|d| format!(", default {}", codegraph_core::extract::truncate(d, 40))).unwrap_or_default();
            let entry = format!("{file}:{line} in {who} ({src}{d})");
            if !e.0.contains(&entry) {
                e.0.push(entry);
            }
        }
    }
    let read_keys = keys.values().filter(|v| !v.0.is_empty()).count();
    let missing = keys.values().filter(|v| !v.0.is_empty() && v.1.is_empty()).count();
    let mut out = Out::new();
    out.h(format!("{} config key(s): {read_keys} read by code, {missing} read but not defined in any indexed config file", keys.len()));
    let json = wants_json(&p.format);
    // Keys read by code first, then definitions nobody reads.
    let mut ordered: Vec<(&String, &(Vec<String>, Vec<String>))> = keys.iter().collect();
    ordered.sort_by_key(|(_, v)| v.0.is_empty());
    for (k, (reads, defs)) in ordered {
        if json {
            out.item(json!({ "key": k, "reads": reads, "defined_at": defs }).to_string());
            continue;
        }
        let mut item = k.to_string();
        if reads.is_empty() {
            item.push_str(&format!(" | defined only: {}", defs.join("; ")));
        } else {
            item.push_str(&format!(" | reads: {}", reads.join("; ")));
            if defs.is_empty() && (k.starts_with("asset:") || k.starts_with("anim:")) {
                item.push_str(" | ⚠ never loaded / created (no `this.load.*` / `anims.create` defines it)");
            } else if defs.is_empty() && k.starts_with("registry:") {
                item.push_str(" | ⚠ read but never set in the registry");
            } else if defs.is_empty() {
                item.push_str(" | ⚠ not defined in config files (must come from the environment)");
            } else {
                item.push_str(&format!(" | defined: {}", defs.join("; ")));
            }
        }
        out.item(item);
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContractParams {
    /// Type id or name (class, interface, struct, data model).
    pub type_id: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_type_contract(ctx: &Ctx, p: ContractParams) -> ToolResult {
    let mut n = resolve_symbol(ctx.conn, &p.type_id)?;
    if !n.sym.kind.is_type() {
        match n.sym.parent_id.clone().and_then(|pid| query::node(ctx.conn, &pid).ok().flatten()) {
            Some(parent) if parent.sym.kind.is_type() => n = parent,
            _ => return Err(ToolError::BadArgs(format!("{} is not a type", display(&n)))),
        }
    }
    let mut out = Out::new();
    out.h(format!("{} {}", n.sym.signature, n.coords()));
    if let Some(d) = &n.sym.docstring {
        out.h(format!("/// {d}"));
    }
    let impls = query::edges_in(ctx.conn, &n.sym.id, &["INHERITS"])?.len();
    if impls > 0 {
        out.h(format!("(implemented/extended by {impls} type(s))"));
    }
    let is_iface = n.sym.kind == SymbolKind::Interface;
    for c in query::children(ctx.conn, &n.sym.id)? {
        if !is_iface && (c.sym.visibility == "private" || c.sym.visibility == "protected") {
            continue;
        }
        let mut prefix = String::new();
        if c.sym.is_static && c.sym.kind != SymbolKind::Const {
            prefix.push_str("static ");
        }
        if c.sym.is_abstract && !is_iface {
            prefix.push_str("abstract ");
        }
        out.item(format!("  {prefix}{} :{}", c.sym.signature, c.sym.start_line));
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BodyParams {
    /// Symbol id or name.
    pub symbol_id: String,
    /// Remove comments (line numbers are preserved).
    pub strip_comments: Option<bool>,
    /// Maximum number of lines to return.
    pub max_lines: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_symbol_body(ctx: &Ctx, p: BodyParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    if n.sym.kind == SymbolKind::Literal {
        return Err(ToolError::BadArgs("literal nodes have no body; use find_literal".into()));
    }
    let path = ctx.engine.root.join(&n.file_path);
    let source = std::fs::read_to_string(&path).map_err(|e| ToolError::Internal(anyhow::anyhow!("reading {}: {e}", n.file_path)))?;
    let strip = p.strip_comments.unwrap_or(false);
    let lang = Language::from_path(&path);
    let text = match (strip, lang) {
        (true, Some(l)) if l.has_tree_sitter() => render::strip_comments(l, &source),
        _ => source.clone(),
    };
    let original: Vec<&str> = source.lines().collect();
    let lines: Vec<&str> = text.lines().collect();
    let start = n.sym.start_line.max(1) as usize;
    let end = (n.sym.end_line as usize).min(lines.len());
    let mut out = Out::new();
    out.h(format!("{} | {}", node_line(&n), n.sym.signature));
    let mut body: Vec<String> = Vec::new();
    for i in start..=end {
        let l = lines.get(i - 1).copied().unwrap_or("");
        if strip && l.trim().is_empty() && !original.get(i - 1).map(|o| o.trim().is_empty()).unwrap_or(true) {
            continue; // line held only a comment
        }
        body.push(format!("{i:>4}| {}", l.trim_end()));
    }
    let total = body.len();
    if let Some(m) = p.max_lines {
        if total > m {
            body.truncate(m);
            body.push(format!("[TRUNCATED: showing {m} of {total} lines]"));
        }
    }
    for b in body {
        out.item(b);
    }
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutlineParams {
    /// File path (relative to the project root; suffixes accepted).
    pub file_path: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_file_outline(ctx: &Ctx, p: OutlineParams) -> ToolResult {
    let file = resolve_file_arg(ctx.conn, &p.file_path)?;
    let nodes = query::nodes_in_file(ctx.conn, &file)?;
    let lang = Language::from_path(std::path::Path::new(&file)).unwrap_or(Language::TypeScript);
    let lines = query::file_line_count(ctx.conn, &file).unwrap_or(0);
    let syms: Vec<codegraph_core::Symbol> = nodes.into_iter().map(|n| n.sym).collect();
    let mut out = Out::new();
    let mut it = render::outline(&file, lines, lang, &syms).into_iter();
    if let Some(h) = it.next() {
        out.h(h);
    }
    for l in it {
        out.item(l);
    }
    // Config files have no symbols but declare keys.
    if out.items.is_empty() {
        let mut st = ctx.conn.prepare("SELECT config_key, line_number FROM config_dependency WHERE file_path = ?1 ORDER BY line_number")?;
        for r in st.query_map(params![file], |r| Ok(format!("key {} :{}", r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?.flatten() {
            out.item(r);
        }
    }
    let _ = (NODE_COLS, node_from_row as fn(&rusqlite::Row) -> rusqlite::Result<codegraph_graph::Node>);
    Ok(out)
}
