//! Read-side helpers shared by the MCP server and the web UI.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;

use crate::store::{edge_from_row, node_from_row, EdgeRow, Node, EDGE_COLS, NODE_COLS};

pub fn node(conn: &Connection, id: &str) -> Result<Option<Node>> {
    Ok(conn.query_row(&format!("SELECT {NODE_COLS} FROM node WHERE id = ?1"), [id], node_from_row).optional()?)
}

pub fn nodes_by_ids(conn: &Connection, ids: &[String]) -> Result<HashMap<String, Node>> {
    let mut out = HashMap::new();
    let uniq: Vec<&String> = ids.iter().collect::<HashSet<_>>().into_iter().collect();
    for chunk in uniq.chunks(400) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE id IN ({ph})"))?;
        let rows = st.query_map(params_from_iter(chunk.iter()), node_from_row)?;
        for n in rows.flatten() {
            out.insert(n.sym.id.clone(), n);
        }
    }
    Ok(out)
}

pub fn nodes_in_file(conn: &Connection, file: &str) -> Result<Vec<Node>> {
    let mut st =
        conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE file_path = ?1 AND derived = 0 ORDER BY start_line, end_line DESC"))?;
    let rows = st.query_map([file], node_from_row)?;
    Ok(rows.flatten().collect())
}

pub fn children(conn: &Connection, parent: &str) -> Result<Vec<Node>> {
    let mut st = conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE parent_id = ?1 ORDER BY start_line"))?;
    let rows = st.query_map([parent], node_from_row)?;
    Ok(rows.flatten().collect())
}

/// Resolves a user-supplied file reference (exact, suffix, or basename) to an indexed path.
pub fn resolve_file(conn: &Connection, input: &str) -> Result<Vec<String>> {
    let norm = input.replace('\\', "/").trim_start_matches("./").to_string();
    let exact: Option<String> = conn.query_row("SELECT file_path FROM file_meta WHERE file_path = ?1", [&norm], |r| r.get(0)).optional()?;
    if let Some(e) = exact {
        return Ok(vec![e]);
    }
    let mut st = conn.prepare("SELECT file_path FROM file_meta WHERE file_path LIKE ?1 ORDER BY length(file_path) LIMIT 20")?;
    let rows = st.query_map([format!("%{norm}")], |r| r.get::<_, String>(0))?;
    let v: Vec<String> =
        rows.flatten().filter(|p| p.ends_with(&norm) && (p.len() == norm.len() || p[..p.len() - norm.len()].ends_with('/'))).collect();
    Ok(v)
}

/// Finds nodes matching an id, qualified name, `Type.member`, name or `file:line` reference.
pub fn find_nodes(conn: &Connection, query: &str, limit: usize) -> Result<Vec<Node>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(n) = node(conn, q)? {
        return Ok(vec![n]);
    }
    // file:line → innermost symbol enclosing the line.
    if let Some((f, l)) = q.rsplit_once(':') {
        if let Ok(line) = l.split('-').next().unwrap_or(l).parse::<u32>() {
            let files = resolve_file(conn, f)?;
            if let Some(file) = files.first() {
                let mut st = conn.prepare(&format!(
                    "SELECT {NODE_COLS} FROM node WHERE file_path = ?1 AND derived = 0 AND start_line <= ?2 AND end_line >= ?2 AND kind <> 'module' ORDER BY (end_line - start_line) LIMIT 1"
                ))?;
                let hit = st.query_row(params![file, line], node_from_row).optional()?;
                if let Some(h) = hit {
                    return Ok(vec![h]);
                }
            }
        }
    }
    let mut out: Vec<Node> = Vec::new();
    let mut seen = HashSet::new();
    let mut push_all = |rows: Vec<Node>, out: &mut Vec<Node>| {
        for n in rows {
            if seen.insert(n.sym.id.clone()) {
                out.push(n);
            }
        }
    };
    let mut st = conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE qualified_name = ?1 LIMIT ?2"))?;
    push_all(st.query_map(params![q, limit as i64], node_from_row)?.flatten().collect(), &mut out);
    if out.is_empty() {
        let mut st = conn.prepare(&format!(
            "SELECT {NODE_COLS} FROM node WHERE qualified_name LIKE ?1 AND kind <> 'literal' ORDER BY length(qualified_name) LIMIT ?2"
        ))?;
        let rows: Vec<Node> = st
            .query_map(params![format!("%.{q}"), limit as i64], node_from_row)?
            .flatten()
            .filter(|n| n.sym.qualified_name.ends_with(&format!(".{q}")))
            .collect();
        push_all(rows, &mut out);
    }
    if out.is_empty() {
        let mut st = conn.prepare(&format!(
            "SELECT {NODE_COLS} FROM node WHERE name = ?1 AND kind NOT IN ('module', 'literal') ORDER BY derived, kind LIMIT ?2"
        ))?;
        push_all(st.query_map(params![q, limit as i64], node_from_row)?.flatten().collect(), &mut out);
    }
    if out.is_empty() {
        let mut st =
            conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE name = ?1 COLLATE NOCASE AND kind <> 'literal' LIMIT ?2"))?;
        push_all(st.query_map(params![q, limit as i64], node_from_row)?.flatten().collect(), &mut out);
    }
    Ok(out)
}

const KIND_RANK: &str = "CASE WHEN kind IN ('class','interface','struct','enum','data_model','type_alias') THEN 0     WHEN kind IN ('function','method','constructor') THEN 1 WHEN kind = 'module' THEN 3 ELSE 2 END";

/// Tiered symbol search: exact name (B-tree, NOCASE) → name prefix → FTS5 trigram
/// substring ranked by bm25. Only the tiers needed to fill `limit` are executed, so
/// common terms stay fast. Returns (rows, total matches; capped at 1000).
pub fn search(conn: &Connection, q: &str, kind: Option<&str>, limit: usize, include_literal_nodes: bool) -> Result<(Vec<Node>, usize)> {
    let q = q.trim();
    let mut filters = String::new();
    let mut extra: Vec<String> = Vec::new();
    if let Some(k) = kind {
        filters.push_str(" AND kind = ?2");
        extra.push(k.to_string());
    } else {
        filters.push_str(" AND ?2 IS NULL");
        extra.push(String::new());
    }
    if !include_literal_nodes {
        filters.push_str(" AND kind <> 'literal'");
    }
    let kind_arg: Option<&str> = kind;
    let mut out: Vec<Node> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut push = |rows: Vec<Node>, out: &mut Vec<Node>| {
        for n in rows {
            if seen.insert(n.sym.id.clone()) {
                out.push(n);
            }
        }
    };
    // 1. Exact name.
    let sql = format!("SELECT {NODE_COLS} FROM node WHERE name = ?1 COLLATE NOCASE{filters} ORDER BY derived, {KIND_RANK}, length(qualified_name) LIMIT ?3");
    let mut st = conn.prepare_cached(&sql)?;
    push(st.query_map(params![q, kind_arg, limit as i64], node_from_row)?.flatten().collect(), &mut out);
    // 2. Name prefix (uses the NOCASE index through the LIKE optimization).
    if out.len() < limit {
        let prefix = format!("{}%", q.replace('!', "!!").replace('%', "!%").replace('_', "!_"));
        let sql = format!("SELECT {NODE_COLS} FROM node WHERE name LIKE ?1 ESCAPE '!'{filters} ORDER BY derived, {KIND_RANK}, length(name), length(qualified_name) LIMIT ?3");
        let mut st = conn.prepare_cached(&sql)?;
        push(st.query_map(params![prefix, kind_arg, (limit * 2) as i64], node_from_row)?.flatten().collect(), &mut out);
    }
    // 3. Substring / qualified-name / signature / path matches via FTS5 trigram.
    let use_fts = q.chars().count() >= 3;
    if out.len() < limit {
        if use_fts {
            let fts_q = format!("\"{}\"", q.replace('"', "\"\""));
            let cols = NODE_COLS.split(", ").map(|c| format!("n.{c}")).collect::<Vec<_>>().join(", ");
            let sql = format!(
                "SELECT {cols} FROM (SELECT rowid, rank FROM node_fts WHERE node_fts MATCH ?1 ORDER BY rank LIMIT ?3) f JOIN node n ON n.rid = f.rowid WHERE 1 = 1{} ORDER BY n.derived, f.rank",
                filters.replace(" kind", " n.kind")
            );
            let mut st = conn.prepare_cached(&sql)?;
            push(st.query_map(params![fts_q, kind_arg, (limit * 4).max(60) as i64], node_from_row)?.flatten().collect(), &mut out);
        } else {
            let sub = format!("%{}%", q.replace('!', "!!").replace('%', "!%").replace('_', "!_"));
            let sql = format!("SELECT {NODE_COLS} FROM node WHERE (name LIKE ?1 ESCAPE '!' OR qualified_name LIKE ?1 ESCAPE '!'){filters} ORDER BY derived, {KIND_RANK}, length(name) LIMIT ?3");
            let mut st = conn.prepare_cached(&sql)?;
            push(st.query_map(params![sub, kind_arg, (limit * 2) as i64], node_from_row)?.flatten().collect(), &mut out);
        }
    }
    // Total (capped so very common terms stay cheap).
    let total: i64 = if use_fts {
        let fts_q = format!("\"{}\"", q.replace('"', "\"\""));
        conn.query_row("SELECT COUNT(*) FROM (SELECT 1 FROM node_fts WHERE node_fts MATCH ?1 LIMIT 1000)", [fts_q], |r| r.get(0))?
    } else {
        out.len() as i64
    };
    let _ = extra;
    let total = (total as usize).max(out.len());
    out.truncate(limit);
    Ok((out, total))
}

pub fn edges_out(conn: &Connection, id: &str, relations: &[&str]) -> Result<Vec<EdgeRow>> {
    edges_dir(conn, "source_id", id, relations)
}

pub fn edges_in(conn: &Connection, id: &str, relations: &[&str]) -> Result<Vec<EdgeRow>> {
    edges_dir(conn, "target_id", id, relations)
}

fn edges_dir(conn: &Connection, col: &str, id: &str, relations: &[&str]) -> Result<Vec<EdgeRow>> {
    let (rel_clause, mut args): (String, Vec<String>) = if relations.is_empty() {
        (String::new(), vec![id.to_string()])
    } else {
        let ph = (0..relations.len()).map(|i| format!("?{}", i + 2)).collect::<Vec<_>>().join(",");
        (format!(" AND relation IN ({ph})"), std::iter::once(id.to_string()).chain(relations.iter().map(|r| r.to_string())).collect())
    };
    let sql = format!("SELECT {EDGE_COLS} FROM edge WHERE {col} = ?1{rel_clause} ORDER BY line_number");
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(params_from_iter(args.drain(..)), edge_from_row)?;
    Ok(rows.flatten().collect())
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct GraphStats {
    pub files: i64,
    pub nodes: i64,
    pub edges: i64,
    pub exact: i64,
    pub inferred: i64,
    pub unresolved: i64,
    pub error_files: i64,
    pub last_indexed_at: Option<i64>,
    pub languages: Vec<(String, i64)>,
}

pub fn stats(conn: &Connection) -> Result<GraphStats> {
    let one = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    let mut langs = Vec::new();
    {
        let mut st = conn.prepare("SELECT language, COUNT(*) FROM file_meta GROUP BY language ORDER BY 2 DESC")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, i64>(1)?)))?;
        langs.extend(rows.flatten());
    }
    Ok(GraphStats {
        files: one("SELECT COUNT(*) FROM file_meta"),
        nodes: one("SELECT COUNT(*) FROM node"),
        edges: one("SELECT COUNT(*) FROM edge"),
        exact: one("SELECT COUNT(*) FROM edge WHERE confidence = 'exact'"),
        inferred: one("SELECT COUNT(*) FROM edge WHERE confidence = 'inferred'"),
        unresolved: one("SELECT COUNT(*) FROM edge WHERE confidence = 'unresolved'"),
        error_files: one("SELECT COUNT(*) FROM file_meta WHERE status = 'error'"),
        last_indexed_at: crate::store::get_meta(conn, "last_indexed_at").and_then(|v| v.parse().ok()),
        languages: langs,
    })
}

pub fn file_line_count(conn: &Connection, file: &str) -> Option<u32> {
    conn.query_row("SELECT line_count FROM file_meta WHERE file_path = ?1", [file], |r| r.get(0)).optional().ok().flatten()
}
