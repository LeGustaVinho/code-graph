//! Versioned SQLite schema (`PRAGMA user_version`).
//!
//! The database is a cache of the source tree: on a version mismatch every table
//! is dropped and rebuilt, which simply triggers a full re-index.

use rusqlite::Connection;

pub const SCHEMA_VERSION: i32 = 6;

const TABLES: &[&str] = &[
    "file_meta",
    "node",
    "node_fts",
    "edge",
    "literal_occurrence",
    "literal_fts",
    "data_model",
    "data_model_field",
    "state_mutation",
    "config_dependency",
    "di_registration",
    "route",
    "api_route",
    "http_call",
    "global_handler",
    "catch_site",
    "import",
    "route_prefix",
    "ref_name",
    "meta",
];

/// Full-text tables, exactly as declared in [`DDL`] (also used to build them elsewhere).
pub const NODE_FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS node_fts USING fts5(name, qualified_name, signature, file_path, content='node', content_rowid='rid', tokenize='trigram');";
pub const LITERAL_FTS_DDL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS literal_fts USING fts5(literal_value, content='literal_occurrence', content_rowid='id', tokenize='trigram');";

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS file_meta (
    file_path       TEXT PRIMARY KEY,
    content_hash    TEXT NOT NULL,
    last_mtime      INTEGER NOT NULL,
    size            INTEGER NOT NULL DEFAULT 0,
    last_indexed_at INTEGER NOT NULL,
    status          TEXT NOT NULL,          -- indexed | stale | error
    language        TEXT,
    line_count      INTEGER,
    error           TEXT,
    facts           TEXT,                   -- JSON references kept for re-resolution without re-parsing
    is_vendor       INTEGER NOT NULL DEFAULT 0  -- third-party code (Assets/Plugins, vendor/, config)
);

CREATE TABLE IF NOT EXISTS node (
    rid                   INTEGER PRIMARY KEY,
    id                    TEXT NOT NULL UNIQUE,
    file_path             TEXT NOT NULL,
    kind                  TEXT NOT NULL,
    orig_kind             TEXT NOT NULL,
    name                  TEXT NOT NULL,
    qualified_name        TEXT NOT NULL,
    signature             TEXT,
    start_line            INTEGER NOT NULL,
    end_line              INTEGER NOT NULL,
    language              TEXT,
    capabilities          TEXT,             -- JSON array of tags
    cyclomatic_complexity INTEGER NOT NULL DEFAULT 0,
    is_entrypoint         INTEGER NOT NULL DEFAULT 0,
    entry_kind            TEXT,
    is_test               INTEGER NOT NULL DEFAULT 0,
    parent_id             TEXT,
    visibility            TEXT,
    is_exported           INTEGER NOT NULL DEFAULT 0,
    is_static             INTEGER NOT NULL DEFAULT 0,
    is_async              INTEGER NOT NULL DEFAULT 0,
    is_abstract           INTEGER NOT NULL DEFAULT 0,
    docstring             TEXT,
    attrs                 TEXT,
    bases                 TEXT,
    type_ref              TEXT,
    value_text            TEXT,
    params                TEXT,
    body_hash             TEXT,
    namespace             TEXT,
    modifiers             TEXT,
    derived               INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS node_file   ON node(file_path);
CREATE INDEX IF NOT EXISTS node_name   ON node(name);
CREATE INDEX IF NOT EXISTS node_name_ci ON node(name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS node_qname  ON node(qualified_name);
CREATE INDEX IF NOT EXISTS node_parent ON node(parent_id);
CREATE INDEX IF NOT EXISTS node_kind   ON node(kind);
CREATE INDEX IF NOT EXISTS node_entry  ON node(is_entrypoint) WHERE is_entrypoint = 1;

CREATE VIRTUAL TABLE IF NOT EXISTS node_fts USING fts5(name, qualified_name, signature, file_path, content='node', content_rowid='rid', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS node_ai AFTER INSERT ON node BEGIN
    INSERT INTO node_fts(rowid, name, qualified_name, signature, file_path)
    VALUES (new.rid, new.name, new.qualified_name, new.signature, new.file_path);
END;
CREATE TRIGGER IF NOT EXISTS node_ad AFTER DELETE ON node BEGIN
    INSERT INTO node_fts(node_fts, rowid, name, qualified_name, signature, file_path)
    VALUES ('delete', old.rid, old.name, old.qualified_name, old.signature, old.file_path);
END;
CREATE TRIGGER IF NOT EXISTS node_au AFTER UPDATE OF name, qualified_name, signature, file_path ON node BEGIN
    INSERT INTO node_fts(node_fts, rowid, name, qualified_name, signature, file_path)
    VALUES ('delete', old.rid, old.name, old.qualified_name, old.signature, old.file_path);
    INSERT INTO node_fts(rowid, name, qualified_name, signature, file_path)
    VALUES (new.rid, new.name, new.qualified_name, new.signature, new.file_path);
END;

CREATE TABLE IF NOT EXISTS edge (
    id                INTEGER PRIMARY KEY,
    source_id         TEXT NOT NULL,
    target_id         TEXT,
    unresolved_symbol TEXT,
    relation          TEXT NOT NULL,
    confidence        TEXT NOT NULL,       -- exact | inferred | unresolved
    evidence          TEXT,
    resolver          TEXT,
    line_number       INTEGER,
    metadata          TEXT,
    source_file       TEXT,
    target_file       TEXT,
    derived           INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS edge_source      ON edge(source_id, relation);
CREATE INDEX IF NOT EXISTS edge_target      ON edge(target_id, relation);
CREATE INDEX IF NOT EXISTS edge_source_file ON edge(source_file, derived);
CREATE INDEX IF NOT EXISTS edge_target_file ON edge(target_file);
CREATE INDEX IF NOT EXISTS edge_relation    ON edge(relation);

CREATE TABLE IF NOT EXISTS literal_occurrence (
    id                INTEGER PRIMARY KEY,
    literal_value     TEXT NOT NULL,
    literal_type      TEXT NOT NULL,
    file_path         TEXT NOT NULL,
    line_number       INTEGER,
    column_number     INTEGER,
    enclosing_node_id TEXT
);
CREATE INDEX IF NOT EXISTS literal_file  ON literal_occurrence(file_path);
CREATE INDEX IF NOT EXISTS literal_value ON literal_occurrence(literal_value);
CREATE VIRTUAL TABLE IF NOT EXISTS literal_fts USING fts5(literal_value, content='literal_occurrence', content_rowid='id', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS literal_ai AFTER INSERT ON literal_occurrence BEGIN
    INSERT INTO literal_fts(rowid, literal_value) VALUES (new.id, new.literal_value);
END;
CREATE TRIGGER IF NOT EXISTS literal_ad AFTER DELETE ON literal_occurrence BEGIN
    INSERT INTO literal_fts(literal_fts, rowid, literal_value) VALUES ('delete', old.id, old.literal_value);
END;

CREATE TABLE IF NOT EXISTS data_model (
    model_node_id TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    framework     TEXT,
    table_name    TEXT
);
CREATE TABLE IF NOT EXISTS data_model_field (
    id                   INTEGER PRIMARY KEY,
    model_node_id        TEXT NOT NULL,
    field_name           TEXT NOT NULL,
    field_type           TEXT,
    is_primary_key       INTEGER NOT NULL DEFAULT 0,
    is_foreign_key       INTEGER NOT NULL DEFAULT 0,
    target_model         TEXT,
    relation_cardinality TEXT,
    line_number          INTEGER
);
CREATE INDEX IF NOT EXISTS dmf_model ON data_model_field(model_node_id);

CREATE TABLE IF NOT EXISTS state_mutation (
    id             INTEGER PRIMARY KEY,
    method_node_id TEXT NOT NULL,
    target_state   TEXT NOT NULL,
    mutation_type  TEXT NOT NULL,          -- local_field | object_field | global_var | database | cache
    line_number    INTEGER,
    file_path      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS mutation_method ON state_mutation(method_node_id);
CREATE INDEX IF NOT EXISTS mutation_file   ON state_mutation(file_path);

CREATE TABLE IF NOT EXISTS config_dependency (
    id                INTEGER PRIMARY KEY,
    config_key        TEXT NOT NULL,
    source_type       TEXT NOT NULL,
    file_path         TEXT NOT NULL,
    line_number       INTEGER,
    default_value     TEXT,
    usage             TEXT NOT NULL,       -- read | define
    enclosing_node_id TEXT
);
CREATE INDEX IF NOT EXISTS config_file ON config_dependency(file_path);
CREATE INDEX IF NOT EXISTS config_key  ON config_dependency(config_key);

CREATE TABLE IF NOT EXISTS di_registration (
    id                INTEGER PRIMARY KEY,
    file_path         TEXT NOT NULL,
    service           TEXT NOT NULL,
    implementation    TEXT,
    lifetime          TEXT,
    framework         TEXT,
    line_number       INTEGER,
    enclosing_node_id TEXT
);
CREATE INDEX IF NOT EXISTS di_file ON di_registration(file_path);

CREATE TABLE IF NOT EXISTS route (
    id          INTEGER PRIMARY KEY,
    file_path   TEXT NOT NULL,
    handler     TEXT NOT NULL,
    http_method TEXT,
    path        TEXT,
    framework   TEXT,
    line_number INTEGER
);
CREATE INDEX IF NOT EXISTS route_file ON route(file_path);

CREATE TABLE IF NOT EXISTS api_route (
    id          INTEGER PRIMARY KEY,
    handler_id  TEXT NOT NULL,
    http_method TEXT,
    path        TEXT,
    normalized  TEXT,
    framework   TEXT,
    file_path   TEXT,
    line_number INTEGER
);
CREATE INDEX IF NOT EXISTS api_route_handler ON api_route(handler_id);

CREATE TABLE IF NOT EXISTS http_call (
    id                INTEGER PRIMARY KEY,
    file_path         TEXT NOT NULL,
    enclosing_node_id TEXT,
    http_method       TEXT,
    url               TEXT,
    client            TEXT,
    line_number       INTEGER
);
CREATE INDEX IF NOT EXISTS http_call_file ON http_call(file_path);

CREATE TABLE IF NOT EXISTS global_handler (
    id          INTEGER PRIMARY KEY,
    file_path   TEXT NOT NULL,
    handler     TEXT NOT NULL,
    exception   TEXT,
    framework   TEXT,
    line_number INTEGER
);
CREATE INDEX IF NOT EXISTS global_handler_file ON global_handler(file_path);

CREATE TABLE IF NOT EXISTS catch_site (
    id                INTEGER PRIMARY KEY,
    file_path         TEXT NOT NULL,
    enclosing_node_id TEXT,
    exception         TEXT,
    line_number       INTEGER
);
CREATE INDEX IF NOT EXISTS catch_file ON catch_site(file_path);
CREATE INDEX IF NOT EXISTS catch_exc  ON catch_site(exception);

CREATE TABLE IF NOT EXISTS import (
    id          INTEGER PRIMARY KEY,
    file_path   TEXT NOT NULL,
    module      TEXT NOT NULL,
    imported    TEXT,
    local       TEXT,
    kind        TEXT,
    line_number INTEGER
);
CREATE INDEX IF NOT EXISTS import_file ON import(file_path);

CREATE TABLE IF NOT EXISTS route_prefix (
    file_path TEXT NOT NULL,
    prefix    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS route_prefix_file ON route_prefix(file_path);

CREATE TABLE IF NOT EXISTS ref_name (
    file_path TEXT NOT NULL,
    name      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ref_name_name ON ref_name(name);
CREATE INDEX IF NOT EXISTS ref_name_file ON ref_name(file_path);
"#;

pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA foreign_keys = OFF;
         PRAGMA cache_size = -4000;
         PRAGMA busy_timeout = 5000;",
    )
}

/// Names of the secondary indexes and the full-text triggers declared in [`DDL`].
fn bulk_objects() -> (Vec<&'static str>, Vec<&'static str>) {
    let name_after = |line: &'static str, prefix: &str| line.trim().strip_prefix(prefix).and_then(|r| r.split_whitespace().next());
    let mut indexes = Vec::new();
    let mut triggers = Vec::new();
    for line in DDL.lines() {
        if let Some(n) = name_after(line, "CREATE INDEX IF NOT EXISTS ") {
            indexes.push(n);
        } else if let Some(n) = name_after(line, "CREATE TRIGGER IF NOT EXISTS ") {
            triggers.push(n);
        }
    }
    (indexes, triggers)
}

/// Bulk load (first index, full rebuild): empties every indexed table and drops the
/// secondary indexes and full-text triggers, so rows go in without per-row index and
/// trigram upkeep. [`end_bulk`] rebuilds them in one pass each. Both run inside the
/// indexing transaction, so a failed run leaves the schema untouched.
pub fn begin_bulk(conn: &Connection) -> rusqlite::Result<()> {
    let (indexes, triggers) = bulk_objects();
    for t in triggers {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {t};"))?;
    }
    for i in indexes {
        conn.execute_batch(&format!("DROP INDEX IF EXISTS {i};"))?;
    }
    // The full-text tables are left alone: [`end_bulk`] replaces their contents wholesale.
    // (FTS5 caches its index structure per transaction, so touching them here would go
    // stale once their shadow tables are overwritten.)
    for t in TABLES {
        if !matches!(*t, "meta" | "node_fts" | "literal_fts") {
            conn.execute_batch(&format!("DELETE FROM \"{t}\";"))?;
        }
    }
    Ok(())
}

/// Recreates what [`begin_bulk`] dropped. Index builds sort with SQLite's worker threads.
/// The full-text indexes come from `fts` (built in parallel during the load) or, without
/// it, are rebuilt from their tables here.
pub fn end_bulk(conn: &Connection, fts: Option<crate::ftsbuild::Builder>, progress: &crate::Progress) -> anyhow::Result<()> {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    conn.execute_batch(&format!("PRAGMA threads = {threads};"))?;
    let t = std::time::Instant::now();
    conn.execute_batch(DDL)?;
    tracing::debug!("  indexes: {} ms", t.elapsed().as_millis());
    progress.tick();
    let t = std::time::Instant::now();
    match fts {
        Some(built) => built.finish_into(conn)?,
        None => conn.execute_batch(
            "INSERT INTO node_fts(node_fts) VALUES ('rebuild');
             INSERT INTO literal_fts(literal_fts) VALUES ('rebuild');",
        )?,
    }
    tracing::debug!("  full-text: {} ms", t.elapsed().as_millis());
    progress.tick();
    conn.execute_batch("PRAGMA threads = 0;")?;
    Ok(())
}

/// Creates (or migrates) the schema. Returns true when the database was (re)created.
pub fn migrate(conn: &Connection) -> rusqlite::Result<bool> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == SCHEMA_VERSION {
        return Ok(false);
    }
    if version != 0 {
        for t in TABLES {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS \"{t}\";"))?;
        }
    }
    conn.execute_batch(DDL)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    Ok(true)
}
