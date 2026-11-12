use codegraph_graph::query::{find_nodes, resolve_file};
use codegraph_graph::{index_project, open, IndexOptions};
use rusqlite::Connection;

#[test]
fn file_suffix_checks_component_boundary_and_literal_characters_before_limit() {
    let conn = Connection::open_in_memory().unwrap();
    codegraph_graph::schema::migrate(&conn).unwrap();
    let insert = |path: &str| {
        conn.execute(
            "INSERT INTO file_meta (file_path, content_hash, last_mtime, last_indexed_at, status) VALUES (?1, '', 0, 0, 'indexed')",
            [path],
        )
        .unwrap();
    };
    for i in 0..25 {
        insert(&format!("x{i:02}entry.ts"));
        insert(&format!("m{i:02}q.ts"));
    }
    for path in ["a/entry.ts", "nested/component/entry.ts", "nested/🦀/m%_.ts"] {
        insert(path);
    }
    assert_eq!(resolve_file(&conn, "entry.ts").unwrap(), ["a/entry.ts", "nested/component/entry.ts"]);
    assert_eq!(resolve_file(&conn, ".\\component\\entry.ts").unwrap(), ["nested/component/entry.ts"]);
    assert_eq!(resolve_file(&conn, "m%_.ts").unwrap(), ["nested/🦀/m%_.ts"]);
    insert("entry.ts");
    assert_eq!(resolve_file(&conn, "./entry.ts").unwrap(), ["entry.ts"]);
}

#[test]
fn file_scoped_symbol_names_prefer_exact_case_and_treat_wildcards_literally() {
    let temp = tempfile::tempdir().unwrap();
    let source = "enum Command { Open }\nfn open() {}\nstruct Worker;\nimpl Worker { fn start_job(&self) {} fn startXjob(&self) {} }\n";
    std::fs::write(temp.path().join("main.rs"), source).unwrap();
    let mut conn = open(temp.path()).unwrap();
    index_project(temp.path(), &mut conn, IndexOptions::default()).unwrap();

    let names = |query| find_nodes(&conn, query, 20).unwrap().into_iter().map(|node| node.sym.qualified_name).collect::<Vec<_>>();
    assert_eq!(names("main.rs:open"), ["open"]);
    assert_eq!(names("main.rs:OPEN"), ["Command.Open"]);
    assert_eq!(names("main.rs:start_job"), ["Worker.start_job"]);
    assert!(names("main.rs:Ope%").is_empty());
}
