//! Shared engine: owns the writer connection, the file watcher and the freshness check
//! that runs before every query.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use codegraph_core::lang::Language;
use codegraph_core::walk::{normalize_rel, IgnoreRules};
use codegraph_graph::{index_paths_with, index_project_with, stale_files, IndexOptions, IndexStats, Progress, ProgressSnapshot};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::Connection;
use serde::Serialize;

/// A pooled read-only connection; returned to the pool on drop.
pub struct PooledConn {
    conn: Option<Connection>,
    pool: Arc<Mutex<Vec<Connection>>>,
}

impl std::ops::Deref for PooledConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().unwrap()
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(c) = self.conn.take() {
            let mut p = self.pool.lock().unwrap();
            if p.len() < 4 {
                p.push(c);
            }
        }
    }
}

pub struct Engine {
    pub root: PathBuf,
    writer: Mutex<Connection>,
    readers: Arc<Mutex<Vec<Connection>>>,
    dirty: Arc<Mutex<BTreeSet<String>>>,
    watcher: Mutex<Option<RecommendedWatcher>>,
    watching: AtomicBool,
    last_full_check: Mutex<Option<Instant>>,
    pub reindexing: AtomicBool,
    pub pending: AtomicUsize,
    pub last_stats: Mutex<Option<IndexStats>>,
    /// Stage and counts of the running (or last) indexing run.
    pub progress: Arc<Progress>,
}

#[derive(Serialize)]
pub struct EngineStatus {
    pub root: String,
    pub watching: bool,
    pub reindexing: bool,
    pub pending_files: usize,
    pub stale_files: Vec<String>,
    pub stats: codegraph_graph::query::GraphStats,
    /// Present while re-indexing.
    pub progress: Option<ProgressSnapshot>,
}

impl Engine {
    /// Opens the database and brings the index up to date.
    pub fn open(root: &Path) -> Result<Arc<Engine>> {
        Engine::open_with(root, Arc::new(Progress::new()))
    }

    /// [`Engine::open`], reporting the initial indexing run to `progress`.
    pub fn open_with(root: &Path, progress: Arc<Progress>) -> Result<Arc<Engine>> {
        let root = codegraph_graph::canonical_root(root);
        let mut conn = codegraph_graph::open(&root)?;
        let stats = index_project_with(&root, &mut conn, IndexOptions::default(), &progress)?;
        tracing::info!(
            "indexed {} files ({} parsed, {} unchanged) in {} ms",
            stats.files_total,
            stats.files_parsed,
            stats.files_unchanged,
            stats.duration_ms
        );
        Ok(Arc::new(Engine {
            root,
            writer: Mutex::new(conn),
            readers: Arc::new(Mutex::new(Vec::new())),
            dirty: Arc::new(Mutex::new(BTreeSet::new())),
            watcher: Mutex::new(None),
            watching: AtomicBool::new(false),
            last_full_check: Mutex::new(Some(Instant::now())),
            reindexing: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            last_stats: Mutex::new(Some(stats)),
            progress,
        }))
    }

    /// Read-only connection from a small pool (WAL readers never block the writer).
    pub fn reader(&self) -> Result<PooledConn> {
        let pooled = self.readers.lock().unwrap().pop();
        let conn = match pooled {
            Some(c) => c,
            None => codegraph_graph::open_read(&self.root)?,
        };
        Ok(PooledConn { conn: Some(conn), pool: self.readers.clone() })
    }

    /// Starts a recursive file watcher that marks changed files as dirty.
    pub fn start_watcher(&self) {
        let dirty = self.dirty.clone();
        let root = self.root.clone();
        // The watcher sees every write (Unity rewrites Library/PackageCache constantly): apply
        // the same rules as discovery before marking anything dirty.
        let rules = IgnoreRules::load(&root);
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let mut d = dirty.lock().unwrap();
            for p in ev.paths {
                let rel = normalize_rel(&root, &p);
                if rel.starts_with("..") || rules.is_ignored(&rel) || rel.starts_with(".codegraph") || rel.starts_with(".git/") {
                    continue;
                }
                if Language::from_path(&p).is_some() {
                    d.insert(rel);
                }
            }
        };
        match notify::recommended_watcher(handler) {
            Ok(mut w) => {
                if w.watch(&self.root, RecursiveMode::Recursive).is_ok() {
                    *self.watcher.lock().unwrap() = Some(w);
                    self.watching.store(true, Ordering::SeqCst);
                }
            }
            Err(e) => tracing::warn!("file watcher unavailable ({e}); falling back to periodic stat checks"),
        }
    }

    pub fn is_watching(&self) -> bool {
        self.watching.load(Ordering::SeqCst)
    }

    pub fn pending_dirty(&self) -> usize {
        self.dirty.lock().unwrap().len()
    }

    /// Freshness check: re-indexes dirty files (watcher), files referenced by the
    /// current request (stat check) and, without a watcher, any stale file.
    pub fn ensure_fresh(&self, hints: &[String]) -> Result<Option<IndexStats>> {
        let mut paths: BTreeSet<String> = std::mem::take(&mut *self.dirty.lock().unwrap());
        {
            let conn = self.writer.lock().unwrap();
            if !hints.is_empty() {
                paths.extend(stale_files(&self.root, &conn, Some(hints))?);
            }
            if !self.is_watching() {
                let mut last = self.last_full_check.lock().unwrap();
                let due = last.map(|t| t.elapsed() > Duration::from_secs(2)).unwrap_or(true);
                if due {
                    paths.extend(stale_files(&self.root, &conn, None)?);
                    *last = Some(Instant::now());
                }
            }
        }
        if paths.is_empty() {
            return Ok(None);
        }
        let list: Vec<String> = paths.into_iter().collect();
        self.reindex_paths(&list).map(Some)
    }

    pub fn reindex_paths(&self, paths: &[String]) -> Result<IndexStats> {
        self.pending.store(paths.len(), Ordering::SeqCst);
        self.reindexing.store(true, Ordering::SeqCst);
        let res = {
            let mut conn = self.writer.lock().unwrap();
            index_paths_with(&self.root, &mut conn, paths, &self.progress)
        };
        self.reindexing.store(false, Ordering::SeqCst);
        self.pending.store(0, Ordering::SeqCst);
        if let Ok(s) = &res {
            *self.last_stats.lock().unwrap() = Some(s.clone());
        }
        res
    }

    pub fn reindex_all(&self, rebuild: bool) -> Result<IndexStats> {
        self.reindexing.store(true, Ordering::SeqCst);
        let res = {
            let mut conn = self.writer.lock().unwrap();
            index_project_with(&self.root, &mut conn, IndexOptions { rebuild }, &self.progress)
        };
        self.reindexing.store(false, Ordering::SeqCst);
        if let Ok(s) = &res {
            *self.last_stats.lock().unwrap() = Some(s.clone());
        }
        res
    }

    pub fn status(&self) -> Result<EngineStatus> {
        let conn = self.reader()?;
        let stale = if self.is_watching() {
            self.dirty.lock().unwrap().iter().cloned().collect()
        } else {
            stale_files(&self.root, &conn, None).unwrap_or_default()
        };
        let reindexing = self.reindexing.load(Ordering::SeqCst);
        Ok(EngineStatus {
            root: self.root.display().to_string(),
            watching: self.is_watching(),
            reindexing,
            progress: reindexing.then(|| self.progress.snapshot()),
            pending_files: self.pending.load(Ordering::SeqCst),
            stale_files: stale,
            stats: codegraph_graph::query::stats(&conn)?,
        })
    }
}
