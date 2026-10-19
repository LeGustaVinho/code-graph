//! Row mapping and per-file write operations.

use std::collections::BTreeSet;
use std::path::Path;

use codegraph_core::{FileFacts, Language, ParamInfo, Symbol, SymbolKind};
use codegraph_semantic::{Confidence, Edge};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};

/// A node as stored in the graph.
#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub file_path: String,
    pub language: String,
    #[serde(flatten)]
    pub sym: Symbol,
}

impl Node {
    pub fn coords(&self) -> String {
        if self.sym.end_line > self.sym.start_line {
            format!("{}:{}-{}", self.file_path, self.sym.start_line, self.sym.end_line)
        } else {
            format!("{}:{}", self.file_path, self.sym.start_line)
        }
    }

    pub fn lang(&self) -> Option<Language> {
        Language::from_path(Path::new(&self.file_path))
    }
}

pub const NODE_COLS: &str = "id, file_path, kind, name, qualified_name, signature, start_line, end_line, language, capabilities, \
    cyclomatic_complexity, is_entrypoint, entry_kind, is_test, parent_id, visibility, is_exported, is_static, is_async, \
    is_abstract, docstring, attrs, bases, type_ref, value_text, params, body_hash, namespace";

fn json_vec(s: Option<String>) -> Vec<String> {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn node_from_row(r: &Row) -> rusqlite::Result<Node> {
    let kind: String = r.get(2)?;
    let caps: BTreeSet<String> = json_vec(r.get(9)?).into_iter().collect();
    let params: Option<ParamInfo> = r.get::<_, Option<String>>(25)?.and_then(|p| serde_json::from_str(&p).ok());
    let sym = Symbol {
        id: r.get(0)?,
        kind: SymbolKind::parse(&kind).unwrap_or(SymbolKind::Variable),
        name: r.get(3)?,
        qualified_name: r.get(4)?,
        signature: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        start_line: r.get(6)?,
        end_line: r.get(7)?,
        parent_id: r.get(14)?,
        visibility: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
        is_exported: r.get(16)?,
        is_static: r.get(17)?,
        is_async: r.get(18)?,
        is_abstract: r.get(19)?,
        capabilities: caps,
        complexity: r.get(10)?,
        is_entrypoint: r.get(11)?,
        entry_kind: r.get(12)?,
        is_test: r.get(13)?,
        docstring: r.get(20)?,
        attrs: json_vec(r.get(21)?),
        bases: json_vec(r.get(22)?),
        type_ref: r.get(23)?,
        value_text: r.get(24)?,
        params,
        body_hash: r.get::<_, Option<String>>(26)?.unwrap_or_default(),
        namespace: r.get(27)?,
    };
    Ok(Node { file_path: r.get(1)?, language: r.get::<_, Option<String>>(8)?.unwrap_or_default(), sym })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EdgeRow {
    pub id: i64,
    pub source_id: String,
    pub target_id: Option<String>,
    pub unresolved_symbol: Option<String>,
    pub relation: String,
    pub confidence: Confidence,
    pub evidence: String,
    pub resolver: String,
    pub line: u32,
    pub metadata: Option<serde_json::Value>,
    pub source_file: String,
    pub target_file: Option<String>,
    pub derived: bool,
}

pub const EDGE_COLS: &str = "id, source_id, target_id, unresolved_symbol, relation, confidence, evidence, resolver, line_number, metadata, source_file, target_file, derived";

pub fn edge_from_row(r: &Row) -> rusqlite::Result<EdgeRow> {
    Ok(EdgeRow {
        id: r.get(0)?,
        source_id: r.get(1)?,
        target_id: r.get(2)?,
        unresolved_symbol: r.get(3)?,
        relation: r.get(4)?,
        confidence: Confidence::parse(&r.get::<_, String>(5)?),
        evidence: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
        resolver: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        line: r.get::<_, Option<u32>>(8)?.unwrap_or(0),
        metadata: r.get::<_, Option<String>>(9)?.and_then(|m| serde_json::from_str(&m).ok()),
        source_file: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
        target_file: r.get(11)?,
        derived: r.get(12)?,
    })
}

/// Row id derived from the node id, probing past slots held by a different node.
fn node_rowid(tx: &Transaction, id: &str) -> rusqlite::Result<i64> {
    let mut probe = tx.prepare_cached("SELECT id FROM node WHERE rid = ?1")?;
    let mut rid = codegraph_core::hash::stable_rowid(&[id.as_bytes()]);
    while let Some(other) = probe.query_row([rid], |r| r.get::<_, String>(0)).optional()? {
        if other == id {
            break;
        }
        rid = codegraph_core::hash::next_rowid(rid);
    }
    Ok(rid)
}

pub fn insert_node(tx: &Transaction, file: &str, lang: &str, s: &Symbol, derived: bool) -> rusqlite::Result<()> {
    let rid = node_rowid(tx, &s.id)?;
    let mut stmt = tx.prepare_cached(
        "INSERT OR REPLACE INTO node (id, file_path, kind, orig_kind, name, qualified_name, signature, start_line, end_line, language, \
         capabilities, cyclomatic_complexity, is_entrypoint, entry_kind, is_test, parent_id, visibility, is_exported, is_static, \
         is_async, is_abstract, docstring, attrs, bases, type_ref, value_text, params, body_hash, namespace, derived, rid) \
         VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
    )?;
    let caps = serde_json::to_string(&s.capabilities.iter().collect::<Vec<_>>()).unwrap_or_default();
    let attrs = if s.attrs.is_empty() { None } else { serde_json::to_string(&s.attrs).ok() };
    let bases = if s.bases.is_empty() { None } else { serde_json::to_string(&s.bases).ok() };
    let params = s.params.as_ref().and_then(|p| serde_json::to_string(p).ok());
    stmt.execute(params![
        s.id,
        file,
        s.kind.as_str(),
        s.name,
        s.qualified_name,
        s.signature,
        s.start_line,
        s.end_line,
        lang,
        caps,
        s.complexity,
        s.is_entrypoint,
        s.entry_kind,
        s.is_test,
        s.parent_id,
        s.visibility,
        s.is_exported,
        s.is_static,
        s.is_async,
        s.is_abstract,
        s.docstring,
        attrs,
        bases,
        s.type_ref,
        s.value_text,
        params,
        s.body_hash,
        s.namespace,
        derived,
        rid,
    ])?;
    Ok(())
}

/// Content-addressed edge id: an incremental update and a full rebuild assign the same ids,
/// so edges sharing every ORDER BY key still come back in the same order.
fn edge_rowid(tx: &Transaction, e: &Edge, meta: Option<&str>) -> rusqlite::Result<i64> {
    let line = e.line.to_le_bytes();
    fn opt(s: Option<&str>) -> &[u8] {
        s.map(|s| s.as_bytes()).unwrap_or(b"\0null")
    }
    let mut id = codegraph_core::hash::stable_rowid(&[
        e.source_id.as_bytes(),
        opt(e.target_id.as_deref()),
        opt(e.unresolved_symbol.as_deref()),
        e.relation.as_bytes(),
        e.confidence.as_str().as_bytes(),
        e.evidence.as_bytes(),
        e.resolver.as_bytes(),
        &line,
        opt(meta),
        e.source_file.as_bytes(),
        opt(e.target_file.as_deref()),
        if e.derived { b"1" } else { b"0" },
    ]);
    // Identical duplicates (and true hash collisions) take the next free slot.
    let mut probe = tx.prepare_cached("SELECT 1 FROM edge WHERE id = ?1")?;
    while probe.exists([id])? {
        id = codegraph_core::hash::next_rowid(id);
    }
    Ok(id)
}

pub fn insert_edge(tx: &Transaction, e: &Edge) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO edge (source_id, target_id, unresolved_symbol, relation, confidence, evidence, resolver, line_number, metadata, source_file, target_file, derived, id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    let meta = e.metadata.as_ref().filter(|m| m.as_object().map(|o| !o.is_empty()).unwrap_or(true)).map(|m| m.to_string());
    let id = edge_rowid(tx, e, meta.as_deref())?;
    stmt.execute(params![
        e.source_id,
        e.target_id,
        e.unresolved_symbol,
        e.relation,
        e.confidence.as_str(),
        e.evidence,
        e.resolver,
        e.line,
        meta,
        e.source_file,
        e.target_file,
        e.derived,
        id,
    ])?;
    Ok(())
}

/// References stored per file so cross-file resolution can be redone without re-parsing.
#[derive(Serialize, Deserialize, Default)]
pub struct StoredFacts {
    pub refs: Vec<codegraph_core::Reference>,
}

pub const PER_FILE_TABLES: &[&str] = &[
    "literal_occurrence",
    "state_mutation",
    "config_dependency",
    "di_registration",
    "route",
    "http_call",
    "global_handler",
    "catch_site",
    "import",
    "route_prefix",
    "ref_name",
];

/// Removes everything owned by `file` (nodes, per-file facts, non-derived outgoing edges).
pub fn delete_file_rows(tx: &Transaction, file: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM node WHERE file_path = ?1 AND derived = 0", [file])?;
    for t in PER_FILE_TABLES {
        tx.execute(&format!("DELETE FROM {t} WHERE file_path = ?1"), [file])?;
    }
    tx.execute("DELETE FROM edge WHERE source_file = ?1 AND derived = 0", [file])?;
    Ok(())
}

pub fn insert_file_facts(tx: &Transaction, f: &FileFacts, hash: &str, mtime: i64, size: u64, now: i64) -> rusqlite::Result<()> {
    let lang = f.language.map(|l| l.as_str()).unwrap_or("");
    for s in &f.symbols {
        insert_node(tx, &f.path, lang, s, false)?;
    }
    {
        let mut st = tx.prepare_cached("INSERT INTO literal_occurrence (literal_value, literal_type, file_path, line_number, column_number, enclosing_node_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for l in &f.literals {
            st.execute(params![l.value, l.literal_type, f.path, l.line, l.column, l.enclosing])?;
        }
    }
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO state_mutation (method_node_id, target_state, mutation_type, line_number, file_path) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for m in &f.mutations {
            st.execute(params![m.enclosing, m.target, m.mutation_type, m.line, f.path])?;
        }
    }
    {
        let mut st = tx.prepare_cached("INSERT INTO config_dependency (config_key, source_type, file_path, line_number, default_value, usage, enclosing_node_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for c in &f.config {
            st.execute(params![c.key, c.source_type, f.path, c.line, c.default_value, c.usage, c.enclosing])?;
        }
    }
    {
        let mut st = tx.prepare_cached("INSERT INTO di_registration (file_path, service, implementation, lifetime, framework, line_number, enclosing_node_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for d in &f.di {
            st.execute(params![f.path, d.service, d.implementation, d.lifetime, d.framework, d.line, d.enclosing])?;
        }
    }
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO route (file_path, handler, http_method, path, framework, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for r in &f.routes {
            st.execute(params![f.path, r.handler, r.http_method, r.path, r.framework, r.line])?;
        }
    }
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO http_call (file_path, enclosing_node_id, http_method, url, client, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for h in &f.http_calls {
            st.execute(params![f.path, h.enclosing, h.http_method, h.url, h.client, h.line])?;
        }
    }
    {
        let mut st = tx.prepare_cached(
            "INSERT INTO global_handler (file_path, handler, exception, framework, line_number) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for g in &f.global_handlers {
            st.execute(params![f.path, g.handler, g.exception, g.framework, g.line])?;
        }
    }
    {
        let mut st =
            tx.prepare_cached("INSERT INTO catch_site (file_path, enclosing_node_id, exception, line_number) VALUES (?1, ?2, ?3, ?4)")?;
        for r in f.refs.iter().filter(|r| r.kind == codegraph_core::RefKind::Catch) {
            st.execute(params![f.path, r.enclosing, r.name, r.line])?;
        }
    }
    {
        let mut st = tx
            .prepare_cached("INSERT INTO import (file_path, module, imported, local, kind, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
        for i in &f.imports {
            let kind = serde_json::to_value(i.kind).ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
            st.execute(params![f.path, i.module, i.imported, i.local, kind, i.line])?;
        }
    }
    {
        let mut st = tx.prepare_cached("INSERT INTO route_prefix (file_path, prefix) VALUES (?1, ?2)")?;
        for p in &f.route_prefixes {
            st.execute(params![f.path, p])?;
        }
    }
    {
        let mut names: BTreeSet<&str> = BTreeSet::new();
        for r in &f.refs {
            names.insert(r.name.as_str());
            if let Some(t) = &r.receiver_type {
                names.insert(t.as_str());
            }
        }
        for i in &f.imports {
            if let Some(n) = &i.imported {
                names.insert(n.as_str());
            }
        }
        for s in &f.symbols {
            for b in &s.bases {
                names.insert(b.as_str());
            }
        }
        let mut st = tx.prepare_cached("INSERT INTO ref_name (file_path, name) VALUES (?1, ?2)")?;
        for n in names {
            st.execute(params![f.path, codegraph_core::extract::clean_type(n)])?;
        }
    }
    let stored = StoredFacts { refs: f.refs.iter().filter(|r| r.kind != codegraph_core::RefKind::Catch).cloned().collect() };
    let facts_json = serde_json::to_string(&stored).unwrap_or_default();
    tx.execute(
        "INSERT OR REPLACE INTO file_meta (file_path, content_hash, last_mtime, size, last_indexed_at, status, language, line_count, error, facts) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'indexed', ?6, ?7, NULL, ?8)",
        params![f.path, hash, mtime, size as i64, now, lang, f.line_count, facts_json],
    )?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
    Ok(())
}
