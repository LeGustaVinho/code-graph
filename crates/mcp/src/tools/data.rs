//! 5.5 Data contracts, configuration & surgical extraction.

use std::collections::{BTreeMap, HashMap};

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
    /// Only reads in files whose path contains this fragment (e.g. "apps/api").
    pub path: Option<String>,
    /// List keys that config files define but no code reads one per line (default: only their names).
    pub include_unread: Option<bool>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

/// One key's reads grouped by (file, enclosing symbol, source, default): `file:12,30 in f (process.env, default 3000)`.
fn grouped_reads(reads: &[(String, u32, String, String, Option<String>)], key: &str) -> Vec<String> {
    // Grouped by (file, symbol, source); the defaults seen on any of those reads are merged.
    let mut order: Vec<(String, String, String)> = Vec::new();
    let mut lines: HashMap<(String, String, String), (Vec<u32>, Vec<String>)> = HashMap::new();
    for (file, line, who, src, default) in reads {
        let k = (file.clone(), who.clone(), src.clone());
        if !lines.contains_key(&k) {
            order.push(k.clone());
        }
        let v = lines.entry(k).or_default();
        if !v.0.contains(line) {
            v.0.push(*line);
        }
        if let Some(d) = default.as_deref().and_then(|d| clean_default(d, key)) {
            if !v.1.contains(&d) {
                v.1.push(d);
            }
        }
    }
    order
        .into_iter()
        .map(|k| {
            let (ls, defaults) = &lines[&k];
            let ls = ls.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(",");
            let (file, who, src) = k;
            let default = (!defaults.is_empty()).then(|| defaults.iter().map(|d| codegraph_core::extract::truncate(d, 60)).collect::<Vec<_>>().join(" | "));
            let d = default.map(|d| format!(", default {d}")).unwrap_or_default();
            let who = if who.is_empty() { String::new() } else { format!(" in {who}") };
            // `process.env` is the common case: only other sources are named.
            let how = match (src.as_str(), d.is_empty()) {
                ("process.env", true) => String::new(),
                ("process.env", false) => format!(" ({})", d.trim_start_matches(", ")),
                _ => format!(" ({src}{d})"),
            };
            format!("{file}:{ls}{who}{how}")
        })
        .collect()
}

/// A readable fallback value: one line, not `undefined`, and not an expression that reads the
/// key itself (`env.X ?? (prod ? required('X') : 'd')` is reported through its own `'d'`).
fn clean_default(d: &str, key: &str) -> Option<String> {
    let d = codegraph_core::extract::collapse_ws(d);
    let d = d.trim();
    let d = if d.starts_with('(') && d.ends_with(')') { &d[1..d.len() - 1] } else { d };
    if d.is_empty() || d == "undefined" || d == "null" || d.contains(key) {
        return None;
    }
    Some(codegraph_core::extract::truncate(d, 60))
}

pub fn get_config_matrix(ctx: &Ctx, p: ConfigParams) -> ToolResult {
    let mut st = ctx.conn.prepare("SELECT config_key, source_type, file_path, line_number, default_value, usage, enclosing_node_id FROM config_dependency ORDER BY config_key, usage DESC, file_path, line_number")?;
    let rows: Vec<(String, String, String, u32, Option<String>, String, Option<String>)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?.flatten().collect();
    let encl = nodes_map(ctx.conn, rows.iter().filter_map(|r| r.6.clone()))?;
    // key → (reads, defining files). Defined values are never printed: `.env` files hold secrets.
    type Reads = Vec<(String, u32, String, String, Option<String>)>;
    let mut keys: BTreeMap<String, (Reads, Vec<String>)> = BTreeMap::new();
    let path = p.path.as_deref().map(|x| x.replace('\\', "/").trim_start_matches("./").to_string());
    for (key, src, file, line, default, usage, enc) in &rows {
        if let Some(f) = &p.filter {
            if !key.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) {
                continue;
            }
        }
        let e = keys.entry(key.clone()).or_default();
        if usage == "define" {
            if !e.1.contains(file) {
                e.1.push(file.clone());
            }
        } else if path.as_deref().map(|p| file.contains(p)).unwrap_or(true) {
            let who = enc.as_ref().and_then(|x| encl.get(x)).map(display).unwrap_or_default();
            e.0.push((file.clone(), *line, who, src.clone(), default.clone()));
        }
    }
    // With a path filter, keys nobody reads there are out of scope.
    if path.is_some() {
        keys.retain(|_, v| !v.0.is_empty());
    }
    let read_keys = keys.values().filter(|v| !v.0.is_empty()).count();
    let missing = keys.values().filter(|v| !v.0.is_empty() && v.1.is_empty()).count();
    let mut out = Out::new();
    out.h(format!(
        "{} config key(s): {read_keys} read by code ({missing} without indexed definitions)",
        keys.len()
    ));
    let json = wants_json(&p.format);
    let mut unread: Vec<&String> = Vec::new();
    if json {
        for (k, (reads, defs)) in &keys {
            if !reads.is_empty() || p.include_unread.unwrap_or(false) {
                out.item(json!({ "key": k, "reads": grouped_reads(reads, k), "defined_in": defs }).to_string());
            } else {
                unread.push(k);
            }
        }
        if !unread.is_empty() {
            out.item(json!({ "unread_key_count": unread.len(), "hint": "include_unread=true for details" }).to_string());
        }
        return Ok(out);
    }
    // Text: keys grouped under the function that reads them (paths and names are not repeated
    // per key); defining config files are numbered once.
    let mut def_files: Vec<&String> = keys.values().filter(|v| !v.0.is_empty()).flat_map(|v| v.1.iter()).collect();
    def_files.sort();
    def_files.dedup();
    if !def_files.is_empty() {
        let legend: Vec<String> = def_files.iter().enumerate().map(|(i, f)| format!("[{}] {f}", i + 1)).collect();
        out.h(format!("defining files: {}", legend.join(", ")));
    }
    // (file, reader) → [(key, lines, defaults, defined in)]
    type Entry = (String, Vec<u32>, Vec<String>, String);
    let mut groups: BTreeMap<(String, String), Vec<Entry>> = BTreeMap::new();
    let mut first_line: HashMap<(String, String), u32> = HashMap::new();
    for (k, (reads, defs)) in &keys {
        if reads.is_empty() {
            if p.include_unread.unwrap_or(false) {
                out.item(format!("{k} | not read by indexed code | defined in: {}", defs.join(", ")));
            } else {
                unread.push(k);
            }
            continue;
        }
        let defined = if !defs.is_empty() {
            let ids: Vec<String> = defs.iter().filter_map(|d| def_files.iter().position(|f| *f == d)).map(|i| (i + 1).to_string()).collect();
            format!(" [{}]", ids.join(","))
        } else if k.starts_with("asset:") || k.starts_with("anim:") {
            " ⚠ never loaded / created (no `this.load.*` / `anims.create` defines it)".to_string()
        } else if k.starts_with("registry:") {
            " ⚠ read but never set in the registry".to_string()
        } else {
            " (no indexed definition)".to_string()
        };
        for (file, line, who, src, default) in reads {
            let g = (file.clone(), if src == "process.env" { who.clone() } else { format!("{who} ({src})") });
            let fl = first_line.entry(g.clone()).or_insert(*line);
            *fl = (*fl).min(*line);
            let entries = groups.entry(g).or_default();
            let e = match entries.iter_mut().position(|e| &e.0 == k) {
                Some(i) => &mut entries[i],
                None => {
                    entries.push((k.clone(), Vec::new(), Vec::new(), defined.clone()));
                    entries.last_mut().unwrap()
                }
            };
            if !e.1.contains(line) {
                e.1.push(*line);
            }
            if let Some(d) = default.as_deref().and_then(|d| clean_default(d, k)) {
                if !e.2.contains(&d) {
                    e.2.push(d);
                }
            }
        }
    }
    let mut ordered: Vec<((String, String), Vec<Entry>)> = groups.into_iter().collect();
    ordered.sort_by_key(|(g, _)| (g.0.clone(), first_line.get(g).copied().unwrap_or(0)));
    for ((file, who), mut entries) in ordered {
        entries.sort_by_key(|e| e.1.iter().min().copied().unwrap_or(0));
        out.item(if who.is_empty() { format!("{file}:") } else { format!("{file} {who}:") });
        for (k, mut ls, defaults, defined) in entries {
            ls.sort();
            let ls = ls.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(",");
            let d = if defaults.is_empty() { String::new() } else { format!(" ={}", defaults.join(" | ")) };
            out.item(format!("  {k}:{ls}{d}{defined}"));
        }
    }
    if !unread.is_empty() {
        let names: Vec<&str> = unread.iter().take(20).map(|k| k.as_str()).collect();
        let more = if unread.len() > 20 { format!(" … +{}", unread.len() - 20) } else { String::new() };
        out.item(format!(
            "{} key(s) defined in config files but not read by indexed code (dynamic reads are not seen; include_unread=true for details): {}{more}",
            unread.len(),
            names.join(", ")
        ));
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
    /// First absolute file line to include (1-based, inclusive); requires end_line.
    pub start_line: Option<u32>,
    /// Last absolute file line to include (1-based, inclusive); requires start_line.
    pub end_line: Option<u32>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_symbol_body(ctx: &Ctx, p: BodyParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    if n.sym.kind == SymbolKind::Literal {
        return Err(ToolError::BadArgs("literal nodes have no body; use find_literal".into()));
    }
    let selection = match (p.start_line, p.end_line) {
        (None, None) => None,
        (Some(from), Some(to)) if from > 0 && from <= to && from >= n.sym.start_line && to <= n.sym.end_line => {
            Some((from as usize, to as usize))
        }
        (Some(from), Some(to)) => {
            return Err(ToolError::BadArgs(format!(
                "invalid absolute file line range {from}..{to}; start_line and end_line must be ordered, positive, and inside symbol {}:{}-{}",
                n.file_path, n.sym.start_line, n.sym.end_line
            )));
        }
        _ => return Err(ToolError::BadArgs("start_line and end_line must be provided together (absolute, inclusive file lines)".into())),
    };
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
    if let Some((from, to)) = selection {
        if to > original.len() || to > lines.len() {
            return Err(ToolError::BadArgs(format!(
                "requested file lines {from}..{to} exceed available source lines ({} in {})",
                original.len(), n.file_path
            )));
        }
    }
    let mut out = Out::new();
    out.h(format!("{} | {}", node_line(&n), n.sym.signature));
    if let Some((from, to)) = selection {
        out.h(format!("file lines {from}..{to} (absolute, inclusive)"));
    }
    // The body's common indentation is dropped (a method nested in a class or a closure would
    // otherwise repeat it on every line); relative indentation is kept.
    let indent = (start..=end)
        .filter_map(|i| lines.get(i - 1))
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut body: Vec<String> = Vec::new();
    let (render_start, render_end) = selection.unwrap_or((start, end));
    for i in render_start..=render_end {
        let l = lines.get(i - 1).copied().unwrap_or("");
        if strip && l.trim().is_empty() && !original.get(i - 1).map(|o| o.trim().is_empty()).unwrap_or(true) {
            continue; // line held only a comment
        }
        if selection.is_some() {
            // A requested slice is an exact source excerpt, including original indentation.
            body.push(format!("{i:>4}| {l}"));
        } else {
            let l = if l.len() >= indent && l.is_char_boundary(indent) { &l[indent..] } else { l.trim_start() };
            body.push(format!("{i:>4}| {}", l.trim_end()));
        }
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
