use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

#[test]
fn file_body_reads_exact_lines_across_declarations() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/entry.ts"),
        "export function first() {\r\n    return 1;\r\n}\r\n\r\n// setup\r\nexport const second = first();\r\n  second;",
    )
    .unwrap();
    let engine = Engine::open(root).unwrap();
    let args = json!({ "file_path": "entry.ts", "start_line": 2, "end_line": 7 });
    let (text, error) = call_tool(&engine, "get_symbol_body", args.clone());
    assert!(!error, "{text}");
    assert_eq!(
        text,
        "src/entry.ts:2-7\n   2|     return 1;\n   3| }\n   4| \n   5| // setup\n   6| export const second = first();\n   7|   second;"
    );

    let (stripped, error) = call_tool(
        &engine,
        "get_symbol_body",
        json!({ "file_path": "src\\entry.ts", "start_line": 2, "end_line": 7, "strip_comments": true }),
    );
    assert!(!error, "{stripped}");
    assert_eq!(stripped, text.replace("   5| // setup\n", ""));

    let (limited, error) =
        call_tool(&engine, "get_symbol_body", json!({ "file_path": "src/entry.ts", "start_line": 2, "end_line": 7, "max_lines": 2 }));
    assert!(!error && limited.ends_with("[TRUNCATED: showing 2 of 6 lines]"), "{limited}");
}

#[test]
fn file_body_validates_selection_index_coverage_and_live_file_size() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let entry = root.join("entry.ts");
    std::fs::write(&entry, "export const value = 1;\n").unwrap();
    std::fs::write(root.join("notes.txt"), "unindexed\n").unwrap();
    let engine = Engine::open(root).unwrap();

    for (args, message) in [
        (json!({ "file_path": "entry.ts", "start_line": 0, "end_line": 1 }), "ordered and positive"),
        (json!({ "file_path": "entry.ts", "start_line": 2, "end_line": 1 }), "ordered and positive"),
        (json!({ "file_path": "entry.ts", "start_line": 1, "end_line": 2 }), "exceed available source lines"),
        (json!({ "file_path": "entry.ts", "start_line": 1 }), "provided together"),
        (json!({ "file_path": "entry.ts" }), "provided together"),
        (json!({ "file_path": "entry.ts", "symbol_id": "value" }), "either symbol_id or file_path"),
        (json!({ "file_path": "../entry.ts", "start_line": 1, "end_line": 1 }), "relative to project root"),
        (json!({ "file_path": "notes.txt", "start_line": 1, "end_line": 1 }), "not indexed"),
    ] {
        let (text, error) = call_tool(&engine, "get_symbol_body", args);
        assert!(error && text.contains(message), "{text}");
    }

    let args = json!({ "file_path": "entry.ts", "start_line": 1, "end_line": 1 });
    std::fs::write(&entry, "export const latest = 42;\n").unwrap();
    let (text, error) = call_tool(&engine, "get_symbol_body", args.clone());
    assert!(!error && text.contains("const latest = 42"), "{text}");

    std::fs::OpenOptions::new().write(true).open(&entry).unwrap().set_len(codegraph_core::walk::MAX_FILE_BYTES + 1).unwrap();
    let (text, error) = call_tool(&engine, "get_symbol_body", args);
    assert!(error && text.contains("byte source limit"), "{text}");
}

#[cfg(unix)]
#[test]
fn file_body_rejects_indexed_paths_replaced_by_outside_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let entry = root.join("entry.ts");
    let outside = temp.path().join("outside.ts");
    std::fs::write(&entry, "export const inside = 1;\n").unwrap();
    std::fs::write(&outside, "export const outside = 567;\n").unwrap();
    let engine = Engine::open(&root).unwrap();
    let hash = || -> String {
        engine.reader().unwrap().query_row("SELECT content_hash FROM file_meta WHERE file_path = 'entry.ts'", [], |r| r.get(0)).unwrap()
    };
    let before = hash();

    std::fs::remove_file(&entry).unwrap();
    std::os::unix::fs::symlink(&outside, &entry).unwrap();
    let (text, error) = call_tool(&engine, "get_symbol_body", json!({ "file_path": "entry.ts", "start_line": 1, "end_line": 1 }));
    assert!(error && text.contains("resolves outside the project root"), "{text}");
    assert_eq!(hash(), before);
}
