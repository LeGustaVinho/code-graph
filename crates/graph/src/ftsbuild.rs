//! Full-text indexes built in parallel during a bulk load.
//!
//! Trigram indexing is the slowest single step of a first index and SQLite has one writer
//! per database, so the two FTS5 indexes are built on their own threads, each into a
//! scratch database next to the graph, while the main thread stores rows and resolves
//! references. Their FTS5 shadow tables are then copied into the graph in a few bulk
//! statements. Row ids are chosen up front so the copied index points at the same rows the
//! writer stores.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use codegraph_core::FileFacts;
use rusqlite::{params, Connection};

use crate::schema::{LITERAL_FTS_DDL, NODE_FTS_DDL};

/// Row ids of a bulk load into empty tables, in insertion order.
#[derive(Default)]
pub struct BulkIds {
    /// `node.rid` per node id (same probing as `store::node_rowid` on an empty table).
    pub node_rid: HashMap<String, i64>,
    /// First `literal_occurrence.id` of each file (ids are consecutive within a file).
    pub literal_start: HashMap<String, i64>,
}

struct NodeDoc {
    rid: i64,
    name: String,
    qualified_name: String,
    signature: String,
    file: String,
}

/// Assigns row ids for `files` (in the order the writer will store them) and starts
/// building both full-text indexes.
pub fn start(db_dir: &Path, files: &[&FileFacts]) -> (BulkIds, Builder) {
    let mut ids = BulkIds::default();
    let mut owner: HashMap<i64, String> = HashMap::new();
    let mut docs: HashMap<i64, NodeDoc> = HashMap::new();
    let mut literals: Vec<(i64, String)> = Vec::new();
    for f in files {
        for s in &f.symbols {
            let rid = match ids.node_rid.get(&s.id) {
                Some(r) => *r,
                None => {
                    let mut rid = codegraph_core::hash::stable_rowid(&[s.id.as_bytes()]);
                    while owner.contains_key(&rid) {
                        rid = codegraph_core::hash::next_rowid(rid);
                    }
                    owner.insert(rid, s.id.clone());
                    ids.node_rid.insert(s.id.clone(), rid);
                    rid
                }
            };
            // `INSERT OR REPLACE`: a repeated id keeps its row, with the last content.
            docs.insert(
                rid,
                NodeDoc {
                    rid,
                    name: s.name.clone(),
                    qualified_name: s.qualified_name.clone(),
                    signature: s.signature.clone(),
                    file: f.path.clone(),
                },
            );
        }
        ids.literal_start.insert(f.path.clone(), literals.len() as i64 + 1);
        for l in &f.literals {
            literals.push((literals.len() as i64 + 1, l.value.clone()));
        }
    }
    let mut docs: Vec<NodeDoc> = docs.into_values().collect();
    docs.sort_by_key(|d| d.rid);
    let node_path = db_dir.join("fts-node.tmp");
    let literal_path = db_dir.join("fts-literal.tmp");
    let node = {
        let path = node_path.clone();
        std::thread::spawn(move || {
            build(&path, NODE_FTS_DDL, |tx| {
                let mut st =
                    tx.prepare("INSERT INTO node_fts(rowid, name, qualified_name, signature, file_path) VALUES (?1, ?2, ?3, ?4, ?5)")?;
                for d in &docs {
                    st.execute(params![d.rid, d.name, d.qualified_name, d.signature, d.file])?;
                }
                Ok(())
            })
        })
    };
    let literal = {
        let path = literal_path.clone();
        std::thread::spawn(move || {
            build(&path, LITERAL_FTS_DDL, |tx| {
                let mut st = tx.prepare("INSERT INTO literal_fts(rowid, literal_value) VALUES (?1, ?2)")?;
                for (id, v) in &literals {
                    st.execute(params![id, v])?;
                }
                Ok(())
            })
        })
    };
    (ids, Builder { parts: vec![("node_fts", node_path, node), ("literal_fts", literal_path, literal)] })
}

fn build(path: &Path, ddl: &str, fill: impl FnOnce(&rusqlite::Transaction) -> rusqlite::Result<()>) -> Result<()> {
    remove_db(path);
    let mut conn = Connection::open(path)?;
    // Scratch file: no journal, no syncing.
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA cache_size = -262144;")?;
    conn.execute_batch(ddl)?;
    let tx = conn.transaction()?;
    fill(&tx)?;
    tx.commit()?;
    Ok(())
}

fn remove_db(path: &Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(path.with_extension("tmp-journal"));
}

pub struct Builder {
    parts: Vec<(&'static str, PathBuf, JoinHandle<Result<()>>)>,
}

impl Builder {
    /// Waits for the builds, adds the nodes derived since (shared literals) and copies the
    /// shadow tables over the indexes of `conn`. Runs inside the indexing transaction,
    /// before anything else there touches the full-text tables.
    pub fn finish_into(self, conn: &Connection) -> Result<()> {
        let mut result = Ok(());
        let mut paths = Vec::new();
        for (table, path, handle) in self.parts {
            paths.push(path.clone());
            let built = handle.join().map_err(|_| anyhow!("{table} build panicked")).and_then(|r| r);
            if result.is_ok() {
                result = built
                    .and_then(|_| if table == "node_fts" { add_derived_nodes(conn, &path) } else { Ok(()) })
                    .and_then(|_| copy_shadow_tables(conn, table, &path));
            }
        }
        for p in &paths {
            remove_db(p);
        }
        result
    }
}

fn add_derived_nodes(conn: &Connection, path: &Path) -> Result<()> {
    let mut dst = Connection::open(path)?;
    dst.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;
    let tx = dst.transaction()?;
    {
        let mut read = conn.prepare("SELECT rid, name, qualified_name, signature, file_path FROM node WHERE derived = 1 ORDER BY rid")?;
        let mut write =
            tx.prepare("INSERT INTO node_fts(rowid, name, qualified_name, signature, file_path) VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut rows = read.query([])?;
        while let Some(r) = rows.next()? {
            write.execute(params![
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// FTS5 keeps an index in plain tables (`<name>_data`, `_idx`, `_docsize`, `_config`);
/// an index built elsewhere with the same definition is moved over by copying their rows
/// (ATTACH is not allowed inside the indexing transaction).
fn copy_shadow_tables(conn: &Connection, table: &str, path: &Path) -> Result<()> {
    let src = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    for shadow in ["data", "idx", "docsize", "config"] {
        let name = format!("{table}_{shadow}");
        conn.execute_batch(&format!("DELETE FROM {name};"))?;
        let mut read = src.prepare(&format!("SELECT * FROM {name}"))?;
        let cols = read.column_count();
        let marks = (1..=cols).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let mut write = conn.prepare(&format!("INSERT INTO {name} VALUES ({marks})"))?;
        let mut rows = read.query([])?;
        while let Some(row) = rows.next()? {
            let values: Vec<rusqlite::types::Value> = (0..cols).map(|i| row.get(i)).collect::<rusqlite::Result<_>>()?;
            write.execute(rusqlite::params_from_iter(values))?;
        }
    }
    Ok(())
}
