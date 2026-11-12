//! 5.2 Discovery: search_universal, find_literal, find_variable_or_const.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use codegraph_core::lang::Language;
use codegraph_graph::query;
use codegraph_graph::store::{node_from_row, NODE_COLS};
use codegraph_graph::Node;
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, node_line, node_line_sig, node_line_sig_relative};
use super::{nodes_map, wants_json, Ctx, Out, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Text to search (substring / trigram match on names, qualified names, signatures and paths).
    pub query: String,
    /// Require a case-sensitive, exact name or qualified-name match instead of fuzzy search (default false).
    pub exact: Option<bool>,
    /// Optional kind filter: class, interface, method, function, variable, const, field, property, data_model, test, enum, module, literal.
    pub kind: Option<String>,
    /// Maximum results (default 20).
    pub limit: Option<usize>,
    /// Indent consecutive hits in the same file and reuse the first hit's path. Preserves rank. Text output only.
    pub group_by_file: Option<bool>,
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

/// Exact declaration lookup is intentionally separate from the ranked fuzzy search:
/// prefix, substring, case-insensitive, and signature matches are not admissible.
fn search_exact(ctx: &Ctx, query: &str, kind: Option<&str>, limit: usize) -> Result<(Vec<Node>, usize), rusqlite::Error> {
    let query = query.trim();
    let kind_filter = if kind.is_some() { "AND kind = ?2" } else { "AND ?2 IS NULL" };
    let literal_filter = if kind == Some("literal") { "" } else { "AND kind <> 'literal'" };
    let predicate = format!("(name = ?1 COLLATE BINARY OR qualified_name = ?1 COLLATE BINARY) {kind_filter} {literal_filter}");
    let sql = format!("SELECT {NODE_COLS} FROM node WHERE {predicate} ORDER BY derived, kind, qualified_name, file_path, start_line, id LIMIT ?3");
    let mut st = ctx.conn.prepare(&sql)?;
    let rows: Vec<Node> = st.query_map(params![query, kind, limit as i64], node_from_row)?.collect::<Result<_, _>>()?;
    let count_sql = format!("SELECT COUNT(*) FROM (SELECT 1 FROM node WHERE {predicate} LIMIT 1000)");
    let total: i64 = ctx.conn.query_row(&count_sql, params![query, kind], |r| r.get(0))?;
    Ok((rows, total as usize))
}

pub fn search_universal(ctx: &Ctx, p: SearchParams) -> ToolResult {
    let t0 = Instant::now();
    let limit = p.limit.unwrap_or(20).clamp(1, 200);
    let exact = p.exact.unwrap_or(false);
    let (rows, total) = if exact {
        search_exact(ctx, &p.query, p.kind.as_deref(), limit)?
    } else {
        let include_literals = p.kind.as_deref() == Some("literal");
        query::search(ctx.conn, &p.query, p.kind.as_deref(), limit, include_literals)?
    };
    let mut out = Out::new();
    let total_label = if total >= 1000 { "1000+".to_string() } else { total.to_string() };
    out.h(format!("{total_label} {}match(es) for '{}' ({:.1} ms)", if exact { "exact declaration " } else { "" }, p.query, t0.elapsed().as_secs_f64() * 1000.0));
    if wants_json(&p.format) {
        for n in &rows {
            out.item(node_json(n));
        }
    } else if p.group_by_file.unwrap_or_default() {
        for run in rows.chunk_by(|a, b| a.file_path == b.file_path) {
            out.item(node_line_sig(&run[0]));
            for n in &run[1..] {
                out.item(node_line_sig_relative(n));
            }
        }
    } else {
        for n in &rows {
            out.item(node_line_sig(n));
        }
    }
    out.unshown = total.saturating_sub(rows.len());
    Ok(out)
}

/// Text search over files recorded by the semantic index. Unlike a symbol query,
/// this also finds references in expressions. Zero matches describe only indexed,
/// readable source files, not ignored or unsupported files. It scans source because
/// the reference-name table is not an exhaustive inverted index for every language.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceSearchParams {
    /// Exact text to find (not a regex).
    pub query: String,
    /// Optional root-relative file or directory; excludes every other path.
    pub path: Option<String>,
    /// Match only whole identifiers/words (default false).
    pub whole_word: Option<bool>,
    /// Case-sensitive comparison (default true).
    pub case_sensitive: Option<bool>,
    /// Maximum matching lines to include (default 30, max 200).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

fn identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn search_text(text: &str, case_sensitive: bool) -> Cow<'_, str> {
    if case_sensitive {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.to_lowercase())
    }
}

fn text_matches(haystack: &str, needle: &str, whole_word: bool) -> bool {
    if !whole_word {
        return haystack.contains(needle);
    }
    haystack.match_indices(needle).any(|(start, _)| {
        let end = start + needle.len();
        !haystack[..start].chars().next_back().is_some_and(identifier_char)
            && !haystack[end..].chars().next().is_some_and(identifier_char)
    })
}

fn matches_scope(file: &str, scope: &str) -> bool {
    if scope.is_empty() {
        return true;
    }
    if cfg!(windows) {
        // Windows paths are normally case-insensitive, including their directory
        // components; respect the filesystem's behavior when filtering the index.
        return file.eq_ignore_ascii_case(scope) || file.get(..scope.len()).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case(scope) && file.as_bytes().get(scope.len()) == Some(&b'/')
        });
    }
    file == scope || file.strip_prefix(scope).is_some_and(|suffix| suffix.starts_with('/'))
}

/// Root-relative scope suggestions from known indexed files (without walking
/// directories excluded from indexing). `Tests` can suggest `Assets/Tests`.
fn scope_suggestions(indexed: &[String], scope: &str) -> Vec<String> {
    if scope.contains('/') || scope.is_empty() {
        return Vec::new();
    }
    let mut suggestions = BTreeSet::new();
    for file in indexed {
        let parts: Vec<_> = file.split('/').collect();
        for i in 0..parts.len() {
            if parts[i].eq_ignore_ascii_case(scope) && i > 0 {
                suggestions.insert(parts[..=i].join("/"));
            }
        }
    }
    suggestions.into_iter().take(3).collect()
}

/// An unindexed directory may genuinely have no supported source files (for
/// example an empty Unity Tests folder). Check its contents before treating it
/// as a query failure. Do not follow symlinks, cross the project root, or assume
/// that an unreadable/very large directory is empty. Ignored or oversized source
/// files still count here: their presence makes index coverage inconclusive.
fn scope_has_no_source_files(root: &Path, dir: &Path) -> std::io::Result<bool> {
    if !dir.is_dir() || !std::fs::canonicalize(dir)?.starts_with(std::fs::canonicalize(root)?) {
        return Ok(false);
    }
    let mut pending = vec![dir.to_path_buf()];
    let mut inspected = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            inspected += 1;
            if inspected > 20_000 {
                return Ok(false); // Bounds metadata work for large unindexed trees.
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                return Ok(false); // The target may contain supported files.
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if !kind.is_file() || Language::from_path(&entry.path()).is_some() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub fn search_source(ctx: &Ctx, p: SourceSearchParams) -> ToolResult {
    let needle = p.query.trim();
    if needle.is_empty() {
        return Err(super::ToolError::BadArgs("query must not be empty".into()));
    }
    let scope = p.path.as_deref().unwrap_or("").trim().replace('\\', "/");
    let scope = scope.trim_start_matches("./").trim_end_matches('/');
    let scope = if scope == "." { "" } else { scope };
    if !scope.is_empty() && !Path::new(scope).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return Err(super::ToolError::BadArgs("path must be relative to project root".into()));
    }
    let scope = Path::new(scope).components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join("/");
    let scope = scope.as_str();
    let mut st = ctx.conn.prepare(
        "SELECT file_path, status FROM file_meta ORDER BY file_path",
    )?;
    let indexed = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    let mut indexed_paths = Vec::new();
    let mut files = 0usize;
    let mut unindexed = 0usize;
    let mut unreadable = 0usize;
    let mut lossy_decoded = 0usize;
    let mut matched = 0usize;
    let limit = p.limit.unwrap_or(30).clamp(1, 200);
    let case_sensitive = p.case_sensitive.unwrap_or(true);
    let search_needle = search_text(needle, case_sensitive);
    let whole_word = p.whole_word.unwrap_or(false);
    let json = wants_json(&p.format);
    let mut out = Out::new();
    for (file, status) in indexed.collect::<Result<Vec<_>, _>>()? {
        if status == "indexed" {
            indexed_paths.push(file.clone());
        }
        if !matches_scope(&file, scope) {
            continue;
        }
        if status != "indexed" {
            unindexed += 1;
            continue;
        }
        files += 1;
        let absolute = ctx.engine.root.join(Path::new(&file));
        let Ok(bytes) = std::fs::read(&absolute) else {
            unreadable += 1;
            continue;
        };
        // Legacy source files may carry a non-UTF-8 comment or string. Preserve
        // searchable ASCII identifiers and report that decoding was lossy.
        let content = String::from_utf8_lossy(&bytes);
        if matches!(content, Cow::Owned(_)) {
            lossy_decoded += 1;
        }
        for (i, line) in content.lines().enumerate() {
            if !text_matches(&search_text(line, case_sensitive), &search_needle, whole_word) {
                continue;
            }
            matched += 1;
            if matched <= limit {
                if json {
                    out.item(json!({
                        "file": file,
                        "line": i + 1,
                        "text": line.trim(),
                    }).to_string());
                } else {
                    out.item(format!("{}:{} | {}", file, i + 1, codegraph_core::extract::truncate(line.trim(), 180)));
                }
            }
        }
    }
    if files == 0 {
        if scope.is_empty() {
            return Err(super::ToolError::NotFound("project has no indexed source files; search_source cannot establish whether text is present".into()));
        }
        let exists = ctx.engine.root.join(scope).exists();
        if exists && unindexed == 0
            && scope_has_no_source_files(&ctx.engine.root, &ctx.engine.root.join(scope)).unwrap_or(false)
        {
            out.h(format!(
                "0 matching line(s) for '{needle}' under {scope}: no supported source files exist in this directory or its subdirectories (0 indexed files scanned)."
            ));
            out.h("This is a verified empty source scope; unsupported file formats were not searched.");
            return Ok(out);
        }
        let mut message = if exists {
            format!("path '{scope}' exists but has no indexed source files ({unindexed} recorded as stale/error). No files were searched and absence cannot be established; check ignore rules, supported source extensions, file-size limits, or indexing errors, or search this directory directly with a file-text search.")
        } else {
            format!("path '{scope}' does not exist relative to the project root; no indexed source files were searched. Use a root-relative path")
        };
        let suggestions = scope_suggestions(&indexed_paths, scope);
        if !suggestions.is_empty() {
            message.push_str(&format!(". Indexed paths with that name: {}", suggestions.join(", ")));
        }
        return Err(super::ToolError::NotFound(message));
    }
    let label = if scope.is_empty() { "." } else { scope };
    out.h(format!("{matched} matching line(s) in indexed source for '{needle}' under {label} ({files} indexed files scanned, {unreadable} unreadable, {lossy_decoded} lossy UTF-8, {unindexed} stale/error files excluded)"));
    if matched == 0 {
        out.h("No matches in the indexed, readable source files searched; ignored, unsupported and oversized files were not checked, so absence outside the index is unverified.");
    }
    out.unshown = matched.saturating_sub(limit);
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
