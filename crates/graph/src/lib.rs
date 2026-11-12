//! CodeGraph storage layer: an embedded SQLite graph (`.codegraph/graph.db`) in WAL
//! mode with FTS5 trigram indexes, plus the incremental indexer.

mod ftsbuild;
pub mod indexer;
pub mod progress;
pub mod query;
pub mod schema;
pub mod store;

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

pub use indexer::{index_paths, index_paths_with, index_project, index_project_with, stale_files, IndexOptions, IndexStats};
pub use progress::{Progress, ProgressSnapshot, Stage};
pub use store::{EdgeRow, Node};

pub const DB_DIR: &str = ".codegraph";
pub const DB_FILE: &str = "graph.db";

/// Interprocess guard for schema migration and graph writes. Unlike a sentinel
/// file, the operating system releases this lock if an indexer crashes.
pub struct WriteLock {
    _file: File,
}

pub fn lock_writer(root: &Path) -> Result<WriteLock> {
    let dir = root.join(DB_DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("writer.lock");
    // Never delete this file: a contender may already have opened its inode.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&path)
        .with_context(|| format!("opening writer lock {}", path.display()))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(WriteLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                anyhow::bail!("timed out acquiring graph writer lock {}", path.display());
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("acquiring graph writer lock {}", path.display()));
            }
        }
    }
}

pub fn db_path(root: &Path) -> PathBuf {
    root.join(DB_DIR).join(DB_FILE)
}

/// Opens (creating / migrating if needed) the project's graph database.
pub fn open(root: &Path) -> Result<Connection> {
    let dir = root.join(DB_DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        let _ = std::fs::write(&gi, "*\n");
    }
    let conn = Connection::open(db_path(root)).context("opening graph database")?;
    schema::apply_pragmas(&conn)?;
    schema::migrate(&conn)?;
    Ok(conn)
}

/// Read-only connection for concurrent readers (WAL allows readers during writes).
pub fn open_read(root: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(db_path(root), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA temp_store = MEMORY; PRAGMA cache_size = -1024;")?;
    Ok(conn)
}

/// Canonical project root (absolute, without Windows verbatim prefix).
pub fn canonical_root(path: &Path) -> PathBuf {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = p.to_string_lossy().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}
