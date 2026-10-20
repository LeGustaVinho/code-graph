//! Shared helpers for the language edge-case suites: index a fixture copy and query
//! symbols / edges by (suffix of) qualified name.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use codegraph_graph::{index_project, open, IndexOptions, IndexStats};
use rusqlite::{Connection, OptionalExtension};

pub fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if e.file_name() == ".codegraph" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &dst.join(e.file_name()));
        } else {
            std::fs::copy(&p, dst.join(e.file_name())).unwrap();
        }
    }
}

pub struct Fixture {
    pub _tmp: tempfile::TempDir,
    pub root: PathBuf,
    pub conn: Connection,
    pub stats: IndexStats,
}

/// Copies `fixtures/<name>` to a temp dir and indexes it.
pub fn index_fixture(name: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name);
    let root = tmp.path().join("proj");
    copy_dir(&src, &root);
    let mut conn = open(&root).unwrap();
    let stats = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    Fixture { _tmp: tmp, root, conn, stats }
}

/// SQL condition: column `col` equals placeholder `?n` exactly or as a dotted suffix
/// (`Order.Ship` matches `Shop.Order.Ship`).
fn m(col: &str, n: usize) -> String {
    format!("({col} = ?{n} OR {col} LIKE '%.' || ?{n})")
}

/// Confidence of the strongest `relation` edge from `from` to `to`, if any.
pub fn edge(conn: &Connection, from: &str, to: &str, relation: &str) -> Option<String> {
    let sql = format!(
        "SELECT e.confidence FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = ?3 AND {} AND {} \
         ORDER BY CASE e.confidence WHEN 'exact' THEN 0 WHEN 'inferred' THEN 1 ELSE 2 END LIMIT 1",
        m("s.qualified_name", 1),
        m("t.qualified_name", 2)
    );
    conn.query_row(&sql, rusqlite::params![from, to, relation], |r| r.get(0)).optional().unwrap()
}

/// Every target (qualified name) of `relation` edges leaving `from`.
pub fn targets(conn: &Connection, from: &str, relation: &str) -> Vec<String> {
    let sql = format!(
        "SELECT DISTINCT t.qualified_name FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = ?2 AND {} ORDER BY 1",
        m("s.qualified_name", 1)
    );
    let mut st = conn.prepare(&sql).unwrap();
    st.query_map(rusqlite::params![from, relation], |r| r.get(0)).unwrap().flatten().collect()
}

/// Signatures (and kinds) of every `relation` target from `from`: tells overloads apart.
pub fn target_sigs(conn: &Connection, from: &str, relation: &str) -> Vec<(String, String)> {
    let sql = format!(
        "SELECT DISTINCT t.kind, COALESCE(t.signature, '') FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = ?2 AND {} ORDER BY 1, 2",
        m("s.qualified_name", 1)
    );
    let mut st = conn.prepare(&sql).unwrap();
    st.query_map(rusqlite::params![from, relation], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect()
}

/// Every source (qualified name) of `relation` edges reaching `to`.
pub fn callers(conn: &Connection, to: &str, relation: &str) -> Vec<String> {
    let sql = format!(
        "SELECT DISTINCT s.qualified_name FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = ?2 AND {} ORDER BY 1",
        m("t.qualified_name", 1)
    );
    let mut st = conn.prepare(&sql).unwrap();
    st.query_map(rusqlite::params![to, relation], |r| r.get(0)).unwrap().flatten().collect()
}

#[derive(Debug, Clone)]
pub struct Sym {
    pub kind: String,
    pub qualified_name: String,
    pub file: String,
    pub start_line: u32,
    pub signature: String,
    pub is_entrypoint: bool,
    pub entry_kind: String,
    pub is_test: bool,
    pub attrs: String,
    pub params: String,
}

/// Symbols whose qualified name matches `name` (exact or dotted suffix).
pub fn syms(conn: &Connection, name: &str) -> Vec<Sym> {
    let sql = format!(
        "SELECT kind, qualified_name, file_path, start_line, COALESCE(signature,''), is_entrypoint, COALESCE(entry_kind,''), \
         is_test, COALESCE(attrs,''), COALESCE(params,'') FROM node WHERE derived = 0 AND {} ORDER BY file_path, start_line",
        m("qualified_name", 1)
    );
    let mut st = conn.prepare(&sql).unwrap();
    st.query_map([name], |r| {
        Ok(Sym {
            kind: r.get(0)?,
            qualified_name: r.get(1)?,
            file: r.get(2)?,
            start_line: r.get(3)?,
            signature: r.get(4)?,
            is_entrypoint: r.get(5)?,
            entry_kind: r.get(6)?,
            is_test: r.get(7)?,
            attrs: r.get(8)?,
            params: r.get(9)?,
        })
    })
    .unwrap()
    .flatten()
    .collect()
}

pub fn sym(conn: &Connection, name: &str) -> Sym {
    let all = syms(conn, name);
    assert!(!all.is_empty(), "no symbol matching {name}");
    all[0].clone()
}

pub fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}
