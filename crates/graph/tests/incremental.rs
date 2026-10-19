//! Incremental indexing, cascade invalidation and referential integrity.

use std::path::{Path, PathBuf};

use codegraph_graph::{index_project, open, IndexOptions};
use rusqlite::Connection;

fn copy_dir(src: &Path, dst: &Path) {
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

fn project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fullstack");
    let root = tmp.path().join("proj");
    copy_dir(&src, &root);
    (tmp, root)
}

/// Bumps mtime-sensitive state reliably (some filesystems have coarse mtimes).
fn write(path: &Path, content: &str) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(path, content).unwrap();
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

fn assert_integrity(conn: &Connection) {
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM edge e WHERE NOT EXISTS (SELECT 1 FROM node n WHERE n.id = e.source_id)"),
        0,
        "dangling edge sources"
    );
    assert_eq!(
        count(
            conn,
            "SELECT COUNT(*) FROM edge e WHERE e.target_id IS NOT NULL AND NOT EXISTS (SELECT 1 FROM node n WHERE n.id = e.target_id)"
        ),
        0,
        "dangling edge targets"
    );
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM edge WHERE target_id IS NULL AND confidence <> 'unresolved'"),
        0,
        "null targets must be unresolved"
    );
    assert_eq!(
        count(
            conn,
            "SELECT COUNT(*) FROM node n WHERE n.derived = 0 AND NOT EXISTS (SELECT 1 FROM file_meta f WHERE f.file_path = n.file_path)"
        ),
        0,
        "nodes without file_meta"
    );
    assert_eq!(count(conn, "SELECT COUNT(*) FROM edge WHERE evidence IS NULL OR evidence = ''"), 0, "every edge carries evidence");
    // FTS index stays in sync with the node table.
    assert_eq!(count(conn, "SELECT COUNT(*) FROM node_fts"), count(conn, "SELECT COUNT(*) FROM node"));
}

#[test]
fn first_index_then_cache_hits() {
    let (_t, root) = project();
    let mut conn = open(&root).unwrap();
    let s1 = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert!(s1.files_parsed >= 25, "{s1:?}");
    assert_eq!(s1.parse_errors, 0);
    assert_integrity(&conn);
    let s2 = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert_eq!(s2.files_parsed, 0, "unchanged files must not be reprocessed");
    assert_eq!(s2.files_hashed, 0, "mtime/size pre-filter avoids even hashing");
    assert_eq!(s2.files_unchanged, s1.files_total);
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, codegraph_graph::schema::SCHEMA_VERSION);
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode.to_ascii_lowercase(), "wal");
}

#[test]
fn touch_without_content_change_is_a_cache_hit() {
    let (_t, root) = project();
    let mut conn = open(&root).unwrap();
    index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    let f = root.join("frontend-react/src/utils/hash.ts");
    let content = std::fs::read_to_string(&f).unwrap();
    write(&f, &content);
    let s = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert_eq!(s.files_parsed, 0, "same Blake3 hash → no re-parse ({s:?})");
}

#[test]
fn only_the_modified_file_is_reprocessed_and_dependents_are_updated() {
    let (_t, root) = project();
    let mut conn = open(&root).unwrap();
    index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    let edge_line = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT t.start_line FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
             WHERE e.relation = 'CALLS' AND s.qualified_name = 'AuthStore.login' AND t.name = 'hash'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(edge_line(&conn), 3);

    // Shift `hash` down by two lines.
    let f = root.join("frontend-react/src/utils/hash.ts");
    let content = std::fs::read_to_string(&f).unwrap();
    write(&f, &format!("// moved\n// down\n{content}"));
    let s = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert_eq!(s.parsed_paths, vec!["frontend-react/src/utils/hash.ts".to_string()]);
    assert!(!s.full_resolution, "cascade must stay incremental: {s:?}");
    assert!(s.affected_files < s.files_total, "{s:?}");
    assert_eq!(edge_line(&conn), 5, "dependent edge points at the new location");
    assert_integrity(&conn);

    // Rename the function: the importer's CALLS edge must disappear (cascade invalidation).
    let content = std::fs::read_to_string(&f).unwrap().replace("export function hash(", "export function digest(");
    write(&f, &content);
    let s = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert_eq!(s.files_parsed, 1);
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id WHERE e.relation = 'CALLS' AND s.qualified_name = 'AuthStore.login' AND e.target_id IS NOT NULL"),
        0
    );
    assert_integrity(&conn);
}

#[test]
fn deleted_files_are_removed_with_their_edges() {
    let (_t, root) = project();
    let mut conn = open(&root).unwrap();
    index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    std::fs::remove_file(root.join("backend-dotnet/Handlers/Dispatcher.cs")).unwrap();
    let s = index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    assert_eq!(s.files_removed, 1);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM node WHERE file_path = 'backend-dotnet/Handlers/Dispatcher.cs'"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM edge WHERE target_file = 'backend-dotnet/Handlers/Dispatcher.cs' OR source_file = 'backend-dotnet/Handlers/Dispatcher.cs'"), 0);
    assert_integrity(&conn);
}

#[test]
fn confidence_levels_carry_evidence() {
    let (_t, root) = project();
    let mut conn = open(&root).unwrap();
    index_project(&root, &mut conn, IndexOptions::default()).unwrap();
    let edge = |rel: &str, src: &str, tgt: &str| -> (String, String) {
        conn.query_row(
            "SELECT e.confidence, e.evidence FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
             WHERE e.relation = ?1 AND s.qualified_name = ?2 AND t.qualified_name = ?3",
            [rel, src, tgt],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or_else(|e| panic!("{rel} {src} -> {tgt}: {e}"))
    };
    // exact: interface call dispatched through the .NET DI container registration.
    let (c, ev) = edge("CALLS", "Shop.Controllers.OrdersController.Cancel", "Shop.Services.OrderService.CancelOrderAsync");
    assert_eq!(c, "exact");
    assert!(ev.contains("DI scoped registration") && ev.contains("Program.cs:12"), "{ev}");
    // inferred: untyped fetch/axios call linked to the ASP.NET route.
    let (c, ev) = edge("HTTP_CALLS", "cancelOrder", "Shop.Controllers.OrdersController.Cancel");
    assert_eq!(c, "inferred");
    assert!(ev.contains("route match"), "{ev}");
    // unresolved: interface with several implementations and no registration.
    let (c, ev): (String, String) = conn
        .query_row("SELECT confidence, evidence FROM edge WHERE relation = 'CALLS' AND unresolved_symbol = 'IHandler.Execute'", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(c, "unresolved");
    assert_eq!(ev, "3 potential implementations of IHandler.Execute");
    // exact: direct import.
    let (c, ev) = edge("CALLS", "AuthStore.login", "hash");
    assert_eq!(c, "exact");
    assert!(ev.contains("direct import from './hash'"));
    // Derived relations exist.
    assert!(count(&conn, "SELECT COUNT(*) FROM edge WHERE relation = 'CAUGHT_BY'") >= 2);
    assert!(count(&conn, "SELECT COUNT(*) FROM edge WHERE relation = 'TESTS'") >= 4);
    assert!(count(&conn, "SELECT COUNT(*) FROM edge WHERE relation = 'RESOLVES_DI'") >= 4);
    assert!(count(&conn, "SELECT COUNT(*) FROM data_model") >= 7);
}
