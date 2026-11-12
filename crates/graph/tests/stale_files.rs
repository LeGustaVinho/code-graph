use std::fs;

use codegraph_graph::{index_project, open, stale_files, IndexOptions};

#[test]
fn targeted_staleness_checks_only_the_requested_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut conn = open(root).unwrap();
    for name in ["changed.ts", "deleted.ts", "fresh.ts", "retimed.ts", "unhinted.ts"] {
        fs::write(root.join(name), "export const value = 1;\n").unwrap();
    }
    index_project(root, &mut conn, IndexOptions::default()).unwrap();

    for name in ["changed.ts", "unhinted.ts", "new.ts", "ignored.ts", "unsupported.txt"] {
        fs::write(root.join(name), "export const value = 12345;\n").unwrap();
    }
    fs::remove_file(root.join("deleted.ts")).unwrap();
    fs::create_dir(root.join("directory.ts")).unwrap();
    fs::write(root.join(".codegraphignore"), "changed.ts\nignored.ts\n").unwrap();
    conn.execute("UPDATE file_meta SET last_mtime = last_mtime - 1 WHERE file_path = 'retimed.ts'", []).unwrap();

    let hints = [
        "unsupported.txt",
        "new.ts",
        "deleted.ts",
        "retimed.ts",
        "ignored.ts",
        "changed.ts",
        "changed.ts",
        "directory.ts",
        "missing.ts",
        "fresh.ts",
    ]
    .map(str::to_owned);
    assert_eq!(stale_files(root, &conn, Some(&hints)).unwrap(), ["changed.ts", "changed.ts", "deleted.ts", "new.ts", "retimed.ts"]);
    assert!(stale_files(root, &conn, Some(&[])).unwrap().is_empty());
    assert_eq!(stale_files(root, &conn, None).unwrap(), ["changed.ts", "deleted.ts", "new.ts", "retimed.ts", "unhinted.ts"]);
}
