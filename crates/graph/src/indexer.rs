//! Incremental indexer: mtime/size pre-filter → parallel Blake3 hashing → parallel
//! Tree-sitter extraction of changed files → cascade invalidation → semantic
//! resolution of affected files → project-wide derived relations.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use codegraph_core::walk::{discover, discover_counting, mtime_of, SourceFile};
use codegraph_core::{extract_file, DiRegistration, FileFacts, GlobalHandler, HttpCall, Import, ImportKind, Language, RouteDecl};
use codegraph_semantic::global::{self, CallRow, HandlerRow, HttpCallRow, LiteralRow, RouteRow, ThrowRow};
use codegraph_semantic::index::{IndexedSymbol, SymbolIndex};
use codegraph_semantic::{models, resolve, Confidence, Edge};
use rayon::prelude::*;
use rusqlite::{params, params_from_iter, Connection, Transaction};
use serde::Serialize;

use crate::progress::{Progress, Stage};
use crate::schema;
use crate::store::{self, node_from_row, StoredFacts, NODE_COLS};

#[derive(Clone, Debug, Default, Serialize)]
pub struct IndexStats {
    pub files_total: usize,
    pub files_scanned: usize,
    pub files_hashed: usize,
    pub files_parsed: usize,
    pub files_unchanged: usize,
    pub files_removed: usize,
    pub parse_errors: usize,
    /// Files that parsed with syntax errors (Tree-sitter recovered; facts may be partial).
    #[serde(default)]
    pub syntax_errors: usize,
    pub affected_files: usize,
    pub full_resolution: bool,
    pub nodes: i64,
    pub edges: i64,
    pub duration_ms: u128,
    pub parsed_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IndexOptions {
    /// Re-parse every file even when its content hash is unchanged.
    pub rebuild: bool,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Indexes the whole project incrementally.
pub fn index_project(root: &Path, conn: &mut Connection, opts: IndexOptions) -> Result<IndexStats> {
    index_project_with(root, conn, opts, &Progress::new())
}

/// [`index_project`], reporting each stage to `progress` (read from other threads).
pub fn index_project_with(root: &Path, conn: &mut Connection, opts: IndexOptions, progress: &Progress) -> Result<IndexStats> {
    progress.begin(Stage::Discover, 0);
    let td = Instant::now();
    let files = {
        let found = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|s| {
            let walk = s.spawn(|| discover_counting(root, &found));
            while !walk.is_finished() {
                progress.set_done(found.load(std::sync::atomic::Ordering::Relaxed));
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            walk.join().unwrap_or_default()
        })
    };
    progress.set_done(files.len());
    tracing::debug!("discover: {} files in {} ms", files.len(), td.elapsed().as_millis());
    let on_disk: HashSet<String> = files.iter().map(|f| f.rel_path.clone()).collect();
    let known: Vec<String> = {
        let mut st = conn.prepare("SELECT file_path FROM file_meta")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    let removed: Vec<String> = known.into_iter().filter(|k| !on_disk.contains(k)).collect();
    // A different extractor build invalidates every cached fact.
    let mut opts = opts;
    if crate::store::get_meta(conn, "extractor_version").as_deref() != Some(codegraph_core::EXTRACTOR_VERSION) {
        opts.rebuild = true;
    }
    let stats = run(root, conn, files, removed, opts, progress);
    progress.begin(Stage::Done, 0);
    let stats = stats?;
    crate::store::set_meta(conn, "extractor_version", codegraph_core::EXTRACTOR_VERSION)?;
    Ok(stats)
}

/// Re-indexes only the given relative paths (freshness micro-reindex).
pub fn index_paths(root: &Path, conn: &mut Connection, rel_paths: &[String]) -> Result<IndexStats> {
    index_paths_with(root, conn, rel_paths, &Progress::new())
}

/// [`index_paths`], reporting each stage to `progress`.
pub fn index_paths_with(root: &Path, conn: &mut Connection, rel_paths: &[String], progress: &Progress) -> Result<IndexStats> {
    let mut candidates = Vec::new();
    let mut removed = Vec::new();
    // Same rules as discovery: .gitignore / .ignore / .codegraphignore, build and engine folders.
    let rules = codegraph_core::walk::IgnoreRules::load(root);
    for rel in rel_paths {
        let abs = root.join(rel);
        match std::fs::metadata(&abs) {
            Ok(meta) if meta.is_file() => {
                if let Some(language) = Language::from_path(&abs) {
                    if rules.is_ignored(rel) {
                        continue;
                    }
                    candidates.push(SourceFile {
                        abs_path: abs,
                        rel_path: rel.clone(),
                        language,
                        mtime: mtime_of(&meta),
                        size: meta.len(),
                    });
                }
            }
            _ => removed.push(rel.clone()),
        }
    }
    let stats = run(root, conn, candidates, removed, IndexOptions::default(), progress);
    progress.begin(Stage::Done, 0);
    stats
}

/// Files whose mtime/size differ from the index (no hashing), plus deleted files.
pub fn stale_files(root: &Path, conn: &Connection, only: Option<&[String]>) -> Result<Vec<String>> {
    let mut meta: HashMap<String, (i64, i64)> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT file_path, last_mtime, size FROM file_meta")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))))?;
        for r in rows.flatten() {
            meta.insert(r.0, r.1);
        }
    }
    let mut stale = Vec::new();
    match only {
        Some(paths) => {
            let rules = codegraph_core::walk::IgnoreRules::load(root);
            for p in paths {
                let abs = root.join(p);
                match (std::fs::metadata(&abs), meta.get(p)) {
                    (Ok(m), Some((mt, sz))) => {
                        if mtime_of(&m) != *mt || m.len() as i64 != *sz {
                            stale.push(p.clone());
                        }
                    }
                    (Ok(m), None) => {
                        if m.is_file() && Language::from_path(&abs).is_some() && !rules.is_ignored(p) {
                            stale.push(p.clone());
                        }
                    }
                    (Err(_), Some(_)) => stale.push(p.clone()),
                    (Err(_), None) => {}
                }
            }
        }
        None => {
            let files = discover(root);
            let mut seen = HashSet::new();
            for f in files {
                seen.insert(f.rel_path.clone());
                match meta.get(&f.rel_path) {
                    Some((mt, sz)) if *mt == f.mtime && *sz == f.size as i64 => {}
                    _ => stale.push(f.rel_path.clone()),
                }
            }
            for k in meta.keys() {
                if !seen.contains(k) {
                    stale.push(k.clone());
                }
            }
        }
    }
    stale.sort();
    Ok(stale)
}

struct Parsed {
    file: SourceFile,
    hash: String,
    facts: std::result::Result<FileFacts, String>,
    /// References kept for resolution, and their JSON for `file_meta.facts` (both built
    /// on the parsing threads; resolution reuses them instead of reading them back).
    stored: StoredFacts,
    facts_json: String,
}

fn run(
    root: &Path,
    conn: &mut Connection,
    candidates: Vec<SourceFile>,
    removed: Vec<String>,
    opts: IndexOptions,
    progress: &Progress,
) -> Result<IndexStats> {
    let t0 = Instant::now();
    let _ = root;
    let mut stats = IndexStats { files_scanned: candidates.len(), files_removed: removed.len(), ..Default::default() };
    let mut meta: HashMap<String, (String, i64, i64)> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT file_path, content_hash, last_mtime, size FROM file_meta")?;
        let rows =
            st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))))?;
        for r in rows.flatten() {
            meta.insert(r.0, r.1);
        }
    }
    let first_index = meta.is_empty();

    // 1. mtime / size pre-filter (cache hit without reading the file).
    let to_hash: Vec<SourceFile> = candidates
        .into_iter()
        .filter(|f| opts.rebuild || meta.get(&f.rel_path).map(|(_, m, s)| *m != f.mtime || *s != f.size as i64).unwrap_or(true))
        .collect();
    stats.files_hashed = to_hash.len();
    stats.files_unchanged = stats.files_scanned - to_hash.len();

    // 2. Parallel Blake3 hashing + 3. parallel parsing of modified files.
    progress.begin(Stage::Read, to_hash.len());
    let results: Vec<(SourceFile, String, Option<Vec<u8>>)> = to_hash
        .into_par_iter()
        .filter_map(|f| {
            progress.tick();
            let bytes = std::fs::read(&f.abs_path).ok()?;
            let hash = codegraph_core::hash::content_hash(&bytes);
            Some((f, hash, Some(bytes)))
        })
        .collect();
    let mut touched = Vec::new();
    let mut to_parse = Vec::new();
    for (f, hash, bytes) in results {
        match meta.get(&f.rel_path) {
            Some((h, _, _)) if *h == hash && !opts.rebuild => touched.push((f, hash)),
            _ => to_parse.push((f, hash, bytes.unwrap_or_default())),
        }
    }
    stats.files_unchanged += touched.len();
    progress.begin(Stage::Parse, to_parse.len());
    // Largest files first (they take longest), one file per task, so no thread is left
    // finishing a big file at the end; results go back to discovery order afterwards
    // (row ids depend on insertion order).
    let mut to_parse: Vec<(usize, _)> = to_parse.into_iter().enumerate().collect();
    to_parse.sort_by_key(|(i, (_, _, bytes))| (std::cmp::Reverse(bytes.len()), *i));
    let mut parsed: Vec<(usize, Parsed)> = extraction_pool().install(|| {
        to_parse
            .into_par_iter()
            .with_max_len(1)
            .map(|(i, (f, hash, bytes))| {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let rel = f.rel_path.clone();
                let lang = f.language;
                let facts =
                    catch_unwind(AssertUnwindSafe(|| extract_file(&rel, lang, &text))).map_err(|_| "extractor panicked".to_string());
                let stored = facts.as_ref().map(StoredFacts::of).unwrap_or_default();
                let facts_json = if facts.is_ok() { serde_json::to_string(&stored).unwrap_or_default() } else { String::new() };
                progress.tick();
                (i, Parsed { file: f, hash, facts, stored, facts_json })
            })
            .collect()
    });
    parsed.sort_by_key(|(i, _)| *i);
    let parsed: Vec<Parsed> = parsed.into_iter().map(|(_, p)| p).collect();
    stats.files_parsed = parsed.len();
    stats.parsed_paths = parsed.iter().map(|p| p.file.rel_path.clone()).collect();

    tracing::debug!("hash+parse: {} ms", t0.elapsed().as_millis());
    let now = now_ms();
    // Bulk load (first index, `--rebuild`, extractor upgrade): every file is (re)written, so
    // the tables are emptied at once and indexes are rebuilt after loading instead of
    // being maintained row by row.
    let bulk = first_index || opts.rebuild;
    // Deleting the old rows of already indexed files costs about as much as inserting;
    // bulk mode empties the tables in one statement each.
    let indexed_before = if bulk { 0 } else { parsed.iter().filter(|p| meta.contains_key(&p.file.rel_path)).count() + removed.len() };
    progress.begin(Stage::Store, indexed_before + parsed.len() + removed.len());
    if bulk {
        conn.execute_batch(&format!("PRAGMA cache_size = {BULK_CACHE_KIB};"))?;
    }
    // A first index fills an empty file: in rollback-journal mode new pages are written
    // once (WAL writes them to the log, then copies them again at checkpoint) and still
    // roll back on failure, since pages past the original end need no journal. Only
    // possible while no other connection is open; otherwise it quietly stays in WAL.
    let rollback_journal = first_index
        && conn
            .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get::<_, String>(0))
            .map(|m| m.eq_ignore_ascii_case("delete"))
            .unwrap_or(false);
    // Bulk load: the full-text indexes are built on other threads meanwhile.
    let (ids, fts) = if bulk {
        let files: Vec<&FileFacts> = parsed.iter().filter_map(|p| p.facts.as_ref().ok()).collect();
        let (ids, fts) = crate::ftsbuild::start(&root.join(crate::DB_DIR), &files);
        (Some(ids), Some(fts))
    } else {
        (None, None)
    };
    let res =
        write_and_resolve(root, conn, &mut stats, Work { parsed, touched, removed, meta, first_index, bulk, now, ids, fts }, progress);
    if rollback_journal {
        conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0))?;
    }
    if bulk {
        conn.execute_batch(&format!("PRAGMA cache_size = {WRITER_CACHE_KIB};"))?;
    }
    res?;
    finish_stats(conn, &mut stats, t0)?;
    Ok(stats)
}

/// Writer page cache during a bulk load (KiB, negative = size in SQLite's convention).
const BULK_CACHE_KIB: i64 = -512 * 1024;
/// Writer page cache otherwise (matches `schema::apply_pragmas`).
const WRITER_CACHE_KIB: i64 = -4000;

struct Work {
    parsed: Vec<Parsed>,
    touched: Vec<(SourceFile, String)>,
    removed: Vec<String>,
    meta: HashMap<String, (String, i64, i64)>,
    first_index: bool,
    bulk: bool,
    now: i64,
    ids: Option<crate::ftsbuild::BulkIds>,
    fts: Option<crate::ftsbuild::Builder>,
}

/// Steps 4–6 in one transaction: store facts, cascade, resolve, project-wide relations.
fn write_and_resolve(root: &Path, conn: &mut Connection, stats: &mut IndexStats, w: Work, progress: &Progress) -> Result<()> {
    let Work { parsed, touched, removed, meta, first_index, bulk, now, ids, fts } = w;
    let tx = conn.transaction()?;
    for (f, _) in &touched {
        tx.execute("UPDATE file_meta SET last_mtime = ?1, size = ?2 WHERE file_path = ?3", params![f.mtime, f.size as i64, f.rel_path])?;
    }
    if parsed.is_empty() && removed.is_empty() {
        tx.commit()?;
        return Ok(());
    }

    // 4. Cascade invalidation (a bulk load re-resolves everything).
    let changed: Vec<String> = parsed.iter().map(|p| p.file.rel_path.clone()).chain(removed.iter().cloned()).collect();
    let tp = Instant::now();
    let (old_names, had_di, dependents) = if bulk {
        schema::begin_bulk(&tx)?;
        (BTreeSet::new(), false, BTreeSet::new())
    } else {
        let old_names = select_strings(&tx, "SELECT DISTINCT name FROM node WHERE derived = 0 AND file_path IN", &changed)?;
        let had_di = !select_strings(&tx, "SELECT DISTINCT file_path FROM di_registration WHERE file_path IN", &changed)?.is_empty();
        let dependents = select_strings(&tx, "SELECT DISTINCT source_file FROM edge WHERE derived = 0 AND target_file IN", &changed)?;
        for f in &changed {
            store::delete_file_rows(&tx, f)?;
            if meta.contains_key(f) {
                progress.tick();
            }
        }
        (old_names, had_di, dependents)
    };
    tracing::debug!("delete: {} ms", tp.elapsed().as_millis());
    let tp = Instant::now();
    for f in &removed {
        if !bulk {
            tx.execute("DELETE FROM file_meta WHERE file_path = ?1", [f])?;
        }
        progress.tick();
    }
    let mut new_names: BTreeSet<String> = BTreeSet::new();
    let mut has_di = false;
    for p in &parsed {
        match &p.facts {
            Ok(facts) => {
                if facts.syntax_errors > 0 {
                    stats.syntax_errors += 1;
                }
                store::insert_file_facts(&tx, facts, &p.facts_json, &p.hash, p.file.mtime, p.file.size, now, ids.as_ref())?;
                new_names.extend(facts.symbols.iter().map(|s| s.name.clone()));
                has_di |= !facts.di.is_empty();
                progress.tick();
            }
            Err(e) => {
                stats.parse_errors += 1;
                tx.execute(
                    "INSERT OR REPLACE INTO file_meta (file_path, content_hash, last_mtime, size, last_indexed_at, status, language, line_count, error, facts) VALUES (?1, ?2, ?3, ?4, ?5, 'error', ?6, 0, ?7, NULL)",
                    params![p.file.rel_path, p.hash, p.file.mtime, p.file.size as i64, now, p.file.language.as_str(), e],
                )?;
                progress.tick();
            }
        }
    }
    // Third-party code (defaults + `.codegraph/config.toml`), re-evaluated for every file.
    {
        let vendor = codegraph_core::walk::VendorRules::load(root);
        let files: Vec<(String, bool)> = {
            let mut st = tx.prepare("SELECT file_path, is_vendor FROM file_meta")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?;
            rows.flatten().collect()
        };
        let mut st = tx.prepare_cached("UPDATE file_meta SET is_vendor = ?2 WHERE file_path = ?1")?;
        for (f, current) in files {
            let v = vendor.is_vendor(&f);
            if v != current {
                st.execute(params![f, v])?;
            }
        }
    }
    tracing::debug!("insert: {} ms", tp.elapsed().as_millis());
    let tp = Instant::now();
    let by_name = if bulk {
        BTreeSet::new()
    } else {
        let names: Vec<String> = old_names.union(&new_names).cloned().collect();
        select_strings(&tx, "SELECT DISTINCT file_path FROM ref_name WHERE name IN", &names)?
    };
    let total_files: i64 = tx.query_row("SELECT COUNT(*) FROM file_meta", [], |r| r.get(0))?;
    let removed_set: HashSet<&String> = removed.iter().collect();
    let mut affected: BTreeSet<String> = BTreeSet::new();
    affected.extend(parsed.iter().filter(|p| p.facts.is_ok()).map(|p| p.file.rel_path.clone()));
    affected.extend(dependents.into_iter());
    affected.extend(by_name.into_iter());
    affected.retain(|f| !removed_set.contains(f));
    let full_resolution = first_index || had_di || has_di || (affected.len() as i64) * 2 > total_files;
    if full_resolution {
        tx.execute("DELETE FROM edge WHERE derived = 0", [])?;
        affected = select_strings(&tx, "SELECT file_path FROM file_meta WHERE status = 'indexed' AND file_path NOT IN", &["".to_string()])?;
    } else {
        for f in &affected {
            tx.execute("DELETE FROM edge WHERE source_file = ?1 AND derived = 0", [f])?;
        }
    }
    stats.affected_files = affected.len();
    stats.full_resolution = full_resolution;

    tracing::debug!("cascade: {} ms ({} affected)", tp.elapsed().as_millis(), affected.len());
    // 5. Semantic resolution of affected files.
    progress.begin(Stage::Resolve, affected.len());
    let tp = Instant::now();
    let assemblies = AssemblyKeys::load(root);
    let mut index = load_index(&tx, &|f: &str| assemblies.key(f))?;
    if !index.js_paths.is_empty() {
        // tsconfig `paths` / `baseUrl` and workspace packages drive JS/TS module resolution.
        index.js = codegraph_semantic::jsconfig::load(root);
    }
    if let Some(asm) = &assemblies.unity {
        // Editor-only assemblies (runtime code never resolves into them).
        index.editor_dirs = asm.editor_dirs();
        // Test code is what the Test Framework runs (test assemblies, NUnit tests), not what
        // a folder is called: Unity projects keep mock data and editor helpers in `Test/`.
        let mut st = tx.prepare_cached("UPDATE node SET is_test = ?2 WHERE id = ?1")?;
        for (id, is_test) in codegraph_semantic::unity::apply_test_files(&mut index, asm) {
            st.execute(params![id, is_test])?;
        }
    }
    tracing::debug!("load_index: {} ms", tp.elapsed().as_millis());
    let tp = Instant::now();
    // References of files parsed in this run are still in memory; the others (dependents
    // re-resolved by the cascade) come back from `file_meta.facts`, decoded in parallel.
    let fresh: HashMap<&str, &StoredFacts> =
        parsed.iter().filter(|p| p.facts.is_ok()).map(|p| (p.file.rel_path.as_str(), &p.stored)).collect();
    let loaded: HashMap<String, StoredFacts> = {
        let mut st = tx.prepare("SELECT facts FROM file_meta WHERE file_path = ?1")?;
        let mut raw = Vec::new();
        for f in affected.iter().filter(|f| !fresh.contains_key(f.as_str())) {
            let json: Option<String> = st.query_row([f], |r| r.get(0)).ok().flatten();
            raw.push((f.clone(), json));
        }
        raw.into_par_iter().map(|(f, json)| (f, json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default())).collect()
    };
    let none = StoredFacts::default();
    let facts: Vec<(&String, &StoredFacts)> =
        affected.iter().map(|f| (f, fresh.get(f.as_str()).copied().or_else(|| loaded.get(f)).unwrap_or(&none))).collect();
    let edges: Vec<Edge> = facts
        .par_iter()
        .flat_map_iter(|(f, sf)| {
            let e = resolve::resolve_file(&index, f, &sf.refs);
            progress.tick();
            e
        })
        .collect();
    tracing::debug!("resolve {} edges: {} ms", edges.len(), tp.elapsed().as_millis());
    let tp = Instant::now();
    store::insert_edges(&tx, &edges, bulk)?;
    tracing::debug!("insert edges: {} ms", tp.elapsed().as_millis());
    // 6. Project-wide derived relations.
    let tp = Instant::now();
    progress.begin(Stage::Link, GLOBAL_STEPS);
    recompute_global(&tx, &index, progress)?;
    tracing::debug!("global: {} ms", tp.elapsed().as_millis());
    let tp = Instant::now();
    // Steps: indexes, full-text (bulk only), commit.
    progress.begin(Stage::Save, if bulk { 3 } else { 1 });
    if bulk {
        schema::end_bulk(&tx, fts, progress)?;
        tracing::debug!("rebuild indexes: {} ms", tp.elapsed().as_millis());
    }
    let tp = Instant::now();
    store::set_meta(&tx, "last_indexed_at", &now.to_string())?;
    tx.commit()?;
    progress.tick();
    tracing::debug!("commit: {} ms", tp.elapsed().as_millis());
    Ok(())
}

/// Extraction runs on a dedicated pool with large stacks: extractors recurse over the
/// syntax tree, and generated / minified sources nest deeply (a stack overflow aborts the
/// process; `catch_unwind` cannot stop it).
fn extraction_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new().stack_size(64 << 20).thread_name(|i| format!("extract-{i}")).build().expect("thread pool")
    })
}

fn finish_stats(conn: &Connection, stats: &mut IndexStats, t0: Instant) -> Result<()> {
    stats.files_total = conn.query_row("SELECT COUNT(*) FROM file_meta", [], |r| r.get::<_, i64>(0))? as usize;
    stats.nodes = conn.query_row("SELECT COUNT(*) FROM node", [], |r| r.get(0))?;
    stats.edges = conn.query_row("SELECT COUNT(*) FROM edge", [], |r| r.get(0))?;
    stats.duration_ms = t0.elapsed().as_millis();
    Ok(())
}

/// Runs `sql IN (...)` in chunks (SQLite parameter limits) and collects the first column.
fn select_strings(tx: &Transaction, sql_prefix: &str, values: &[String]) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if values.is_empty() {
        return Ok(out);
    }
    for chunk in values.chunks(500) {
        let placeholders = vec!["?"; chunk.len()].join(",");
        let sql = format!("{sql_prefix} ({placeholders})");
        let mut st = tx.prepare(&sql)?;
        let rows = st.query_map(params_from_iter(chunk.iter()), |r| r.get::<_, String>(0))?;
        for r in rows.flatten() {
            out.insert(r);
        }
    }
    Ok(out)
}

/// Loads the symbol index (all non-derived nodes, imports and DI registrations).
/// Names the assembly compiling a C# file (partial types only merge within one): the
/// owning `.asmdef` or Unity predefined assembly, otherwise the nearest `.csproj` folder.
struct AssemblyKeys {
    root: std::path::PathBuf,
    unity: Option<codegraph_semantic::unity::Assemblies>,
    csproj: std::sync::Mutex<HashMap<String, bool>>,
}

impl AssemblyKeys {
    fn load(root: &Path) -> AssemblyKeys {
        let unity = codegraph_core::walk::is_unity_project(root).then(|| codegraph_semantic::unity::load_assemblies(root));
        AssemblyKeys { root: root.to_path_buf(), unity, csproj: Default::default() }
    }

    fn key(&self, file: &str) -> String {
        if let Some(asm) = &self.unity {
            if let Some(def) = asm.owner(file) {
                return def.name.clone();
            }
            // Unity's predefined assemblies (Assembly-CSharp[-Editor][-firstpass]).
            let editor = file.split('/').rev().skip(1).any(|s| s == "Editor");
            let firstpass =
                ["Assets/Plugins/", "Assets/Standard Assets/", "Assets/Pro Standard Assets/"].iter().any(|p| file.starts_with(p));
            return format!("Assembly-CSharp{}{}", if editor { "-Editor" } else { "" }, if firstpass { "-firstpass" } else { "" });
        }
        let mut dir = file.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        loop {
            if self.has_csproj(dir) || dir.is_empty() {
                return dir.to_string();
            }
            dir = dir.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        }
    }

    fn has_csproj(&self, dir: &str) -> bool {
        if let Some(v) = self.csproj.lock().unwrap().get(dir) {
            return *v;
        }
        let found = std::fs::read_dir(self.root.join(dir))
            .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".csproj")))
            .unwrap_or(false);
        self.csproj.lock().unwrap().insert(dir.to_string(), found);
        found
    }
}

pub fn load_index(conn: &Connection, assembly_of: &dyn Fn(&str) -> String) -> Result<SymbolIndex> {
    let mut syms = Vec::new();
    {
        let mut st = conn.prepare(&format!("SELECT {NODE_COLS} FROM node WHERE derived = 0 ORDER BY file_path, start_line, id"))?;
        let rows = st.query_map([], node_from_row)?;
        for r in rows.flatten() {
            let lang = Language::from_path(Path::new(&r.file_path)).unwrap_or(Language::Config);
            let mut sym = r.sym;
            // Data-model promotion is a derived property; resolve against the declared kind.
            if sym.kind == codegraph_core::SymbolKind::DataModel && !sym.attrs.iter().any(|a| a.starts_with("prisma:")) {
                sym.kind = codegraph_core::SymbolKind::Class;
            }
            syms.push(IndexedSymbol { file: r.file_path, lang, sym });
        }
    }
    let mut imports: HashMap<String, Vec<Import>> = HashMap::new();
    {
        let mut st = conn.prepare("SELECT file_path, module, imported, local, kind, line_number FROM import ORDER BY file_path, id")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<u32>>(5)?,
            ))
        })?;
        for (file, module, imported, local, kind, line) in rows.flatten() {
            let kind: ImportKind =
                kind.and_then(|k| serde_json::from_value(serde_json::Value::String(k)).ok()).unwrap_or(ImportKind::Named);
            imports.entry(file).or_default().push(Import { module, imported, local, kind, line: line.unwrap_or(0) });
        }
    }
    let mut di = Vec::new();
    {
        let mut st = conn.prepare(
            "SELECT file_path, service, implementation, lifetime, framework, line_number, enclosing_node_id FROM di_registration ORDER BY file_path, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                DiRegistration {
                    service: r.get(1)?,
                    implementation: r.get(2)?,
                    lifetime: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    framework: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    line: r.get::<_, Option<u32>>(5)?.unwrap_or(0),
                    enclosing: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                },
            ))
        })?;
        di.extend(rows.flatten());
    }
    Ok(SymbolIndex::build(syms, imports, di, assembly_of))
}

fn load_call_rows(tx: &Transaction) -> Result<Vec<CallRow>> {
    let mut st = tx.prepare(
        "SELECT source_id, target_id, confidence, line_number, metadata, source_file FROM edge WHERE relation = 'CALLS' AND target_id IS NOT NULL \
         ORDER BY source_file, line_number, source_id, target_id, confidence, metadata",
    )?;
    let rows = st.query_map([], |r| {
        let meta: Option<String> = r.get(4)?;
        let try_types: Vec<String> = meta
            .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
            .and_then(|v| v.get("try").and_then(|t| serde_json::from_value(t.clone()).ok()))
            .unwrap_or_default();
        Ok(CallRow {
            source_id: r.get(0)?,
            target_id: r.get(1)?,
            confidence: Confidence::parse(&r.get::<_, String>(2)?),
            line: r.get::<_, Option<u32>>(3)?.unwrap_or(0),
            try_types,
            source_file: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        })
    })?;
    Ok(rows.flatten().collect())
}

/// Progress steps of [`recompute_global`].
const GLOBAL_STEPS: usize = 7;

fn recompute_global(tx: &Transaction, index: &SymbolIndex, progress: &Progress) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM edge WHERE derived = 1;
         DELETE FROM node WHERE derived = 1;
         DELETE FROM api_route;
         DELETE FROM data_model;
         DELETE FROM data_model_field;
         UPDATE node SET kind = orig_kind WHERE kind <> orig_kind;
         UPDATE node SET is_entrypoint = 0, entry_kind = NULL WHERE entry_kind LIKE 'unity message %';",
    )?;
    // Unity messages (`Update`, `OnPointerClick`, …) confirmed through the real inheritance
    // chain: base classes may live in other files, so this is a project-wide property.
    {
        let mut st = tx.prepare_cached("UPDATE node SET is_entrypoint = 1, entry_kind = ?2 WHERE id = ?1")?;
        for (id, kind) in codegraph_semantic::unity::entrypoints(index) {
            st.execute(params![id, kind])?;
        }
    }
    let mut edges: Vec<Edge> = Vec::new();

    let tg = Instant::now();
    // Dependency injection.
    edges.extend(global::di_edges(index));
    tracing::debug!("  di: {} ms", tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();

    // Fullstack routes.
    let routes: Vec<RouteRow> = {
        let mut st =
            tx.prepare("SELECT file_path, handler, http_method, path, framework, line_number FROM route ORDER BY file_path, id")?;
        let rows = st.query_map([], |r| {
            Ok(RouteRow {
                file: r.get(0)?,
                decl: RouteDecl {
                    handler: r.get(1)?,
                    http_method: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    path: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    framework: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    line: r.get::<_, Option<u32>>(5)?.unwrap_or(0),
                },
            })
        })?;
        rows.flatten().collect()
    };
    let prefixes: Vec<(String, String)> = {
        let mut st = tx.prepare("SELECT file_path, prefix FROM route_prefix ORDER BY file_path, prefix")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.flatten().collect()
    };
    let resolved = global::resolve_routes(index, &routes, &prefixes);
    {
        let mut st = tx.prepare_cached("INSERT INTO api_route (handler_id, http_method, path, normalized, framework, file_path, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?;
        for r in &resolved {
            st.execute(params![r.handler_id, r.http_method, r.path, r.normalized, r.framework, r.file, r.line])?;
        }
    }
    let calls: Vec<HttpCallRow> = {
        let mut st =
            tx.prepare("SELECT file_path, enclosing_node_id, http_method, url, client, line_number FROM http_call ORDER BY file_path, id")?;
        let rows = st.query_map([], |r| {
            Ok(HttpCallRow {
                file: r.get(0)?,
                call: HttpCall {
                    enclosing: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    http_method: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    url: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    client: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    line: r.get::<_, Option<u32>>(5)?.unwrap_or(0),
                },
            })
        })?;
        rows.flatten().collect()
    };
    edges.extend(global::http_edges(index, &resolved, &calls));
    tracing::debug!("  http: {} ms", tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();

    // Tests and error flow (both derived from the per-file CALLS / THROWS edges).
    let call_rows = load_call_rows(tx)?;
    edges.extend(global::test_edges(index, &call_rows));
    tracing::debug!("  tests: {} ms", tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();
    let throws: Vec<ThrowRow> = {
        let mut st = tx.prepare(
            "SELECT e.source_id, e.source_file, COALESCE(n.name, e.unresolved_symbol), e.line_number, e.metadata \
             FROM edge e LEFT JOIN node n ON n.id = e.target_id WHERE e.relation = 'THROWS' \
             ORDER BY e.source_file, e.line_number, e.source_id, 3, e.metadata",
        )?;
        let rows = st.query_map([], |r| {
            let meta: Option<String> = r.get(4)?;
            let try_types: Vec<String> = meta
                .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
                .and_then(|v| v.get("try").and_then(|t| serde_json::from_value(t.clone()).ok()))
                .unwrap_or_default();
            Ok(ThrowRow {
                source_id: r.get(0)?,
                source_file: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                exception: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                line: r.get::<_, Option<u32>>(3)?.unwrap_or(0),
                try_types,
            })
        })?;
        rows.flatten().collect()
    };
    let handlers: Vec<HandlerRow> = {
        let mut st =
            tx.prepare("SELECT file_path, handler, exception, framework, line_number FROM global_handler ORDER BY file_path, id")?;
        let rows = st.query_map([], |r| {
            Ok(HandlerRow {
                file: r.get(0)?,
                handler: GlobalHandler {
                    handler: r.get(1)?,
                    exception: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "*".into()),
                    framework: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    line: r.get::<_, Option<u32>>(4)?.unwrap_or(0),
                },
            })
        })?;
        rows.flatten().collect()
    };
    edges.extend(global::caught_by_edges(index, &throws, &call_rows, &handlers));
    tracing::debug!("  caught_by: {} ms", tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();

    // Shared literals.
    let lits: Vec<LiteralRow> = {
        let mut st = tx.prepare("SELECT literal_value, file_path, line_number, enclosing_node_id FROM literal_occurrence WHERE literal_type IN ('string', 'template') ORDER BY file_path, id")?;
        let rows = st.query_map([], |r| {
            Ok(LiteralRow {
                value: r.get(0)?,
                file: r.get(1)?,
                line: r.get::<_, Option<u32>>(2)?.unwrap_or(0),
                enclosing: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
            })
        })?;
        rows.flatten().collect()
    };
    let (lit_nodes, lit_edges) = global::literal_graph(&lits, 2000);
    for (file, sym) in &lit_nodes {
        store::insert_node(tx, file, "literal", sym, true)?;
    }
    edges.extend(lit_edges);
    tracing::debug!("  literals: {} ms", tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();

    for e in &edges {
        store::insert_edge(tx, e)?;
    }

    tracing::debug!("  insert {} derived edges: {} ms", edges.len(), tg.elapsed().as_millis());
    progress.tick();
    let tg = Instant::now();
    // Data models / ERD.
    let (models, fields) = models::extract_models(index);
    tracing::debug!("  models: {} ms", tg.elapsed().as_millis());
    progress.tick();
    {
        let mut st =
            tx.prepare_cached("INSERT OR REPLACE INTO data_model (model_node_id, name, framework, table_name) VALUES (?1, ?2, ?3, ?4)")?;
        let mut up = tx.prepare_cached("UPDATE node SET kind = 'data_model' WHERE id = ?1")?;
        for m in &models {
            st.execute(params![m.id, m.name, m.framework, m.table])?;
            up.execute([&m.id])?;
        }
        let mut st = tx.prepare_cached(
            "INSERT INTO data_model_field (model_node_id, field_name, field_type, is_primary_key, is_foreign_key, target_model, relation_cardinality, line_number) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for f in &fields {
            st.execute(params![
                f.model_id,
                f.field_name,
                f.field_type,
                f.is_primary_key,
                f.is_foreign_key,
                f.target_model,
                f.cardinality,
                f.line
            ])?;
        }
    }
    Ok(())
}
