use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

#[test]
fn batch_refreshes_child_file_and_symbol_hints_in_one_update_without_watcher() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let alpha = root.join("alpha.ts");
    let beta = root.join("beta.ts");
    std::fs::write(&alpha, "export function oldAlpha() { return 1; }\n").unwrap();
    std::fs::write(&beta, "export const oldBeta = 2;\n").unwrap();
    let engine = Engine::open(root).unwrap();

    std::fs::write(&alpha, "// moved\nexport function newAlpha() { return 42; }\n").unwrap();
    std::fs::write(&beta, "// moved\nexport const newBeta = 82;\n").unwrap();
    let (text, error) = call_tool(
        &engine,
        "batch_query",
        json!({
            "operations": [
                {"tool":"get_symbol_body","args":{"symbol_id":"alpha.ts:newAlpha"}},
                {"tool":"get_file_outline","args":{"file_path":"beta.ts","names":["newBeta"]}},
            ]
        }),
    );
    assert!(!error && !text.contains("ERROR:"), "{text}");
    assert!(text.contains("newAlpha alpha.ts:2") && text.contains("return 42;"), "{text}");
    assert!(text.contains("newBeta"), "{text}");
    let last = engine.last_stats.lock().unwrap();
    assert_eq!(last.as_ref().unwrap().parsed_paths, ["alpha.ts", "beta.ts"]);
}

#[test]
fn batch_live_range_unknown_and_nested_operations_add_no_targeted_hints() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let file = root.join("entry.ts");
    std::fs::write(&file, "export const old = 1;\n").unwrap();
    let engine = Engine::open(root).unwrap();
    let index_hash = || -> String {
        engine.reader().unwrap().query_row("SELECT content_hash FROM file_meta WHERE file_path = 'entry.ts'", [], |r| r.get(0)).unwrap()
    };
    let before = index_hash();

    std::fs::write(&file, "export const latest = 42;\n").unwrap();
    let (text, error) = call_tool(
        &engine,
        "batch_query",
        json!({
            "operations": [
                {"tool":"get_symbol_body","args":{"file_path":"entry.ts","start_line":1,"end_line":1}},
                {"tool":"unknown","args":{"file_path":"entry.ts"}},
                {"tool":"batch_query","args":{"operations":[{"tool":"get_file_outline","args":{"file_path":"entry.ts"}}]}},
            ]
        }),
    );
    assert!(!error && text.contains("export const latest = 42;"), "{text}");
    assert!(text.contains("unknown tool 'unknown'") && text.contains("nested batch_query is not allowed"), "{text}");
    assert_eq!(index_hash(), before, "live file ranges read current bytes without targeting the file for reindexing");

    let outline = json!({"tool":"get_file_outline","args":{"file_path":"entry.ts"}});
    for malformed in [
        json!({"operations":[outline], "unexpected":true}),
        json!({
            "operations": [
                outline,
                {"tool":"get_file_outline","args":{"file_path":"entry.ts"},"unexpected":true},
            ]
        }),
    ] {
        let (text, error) = call_tool(&engine, "batch_query", malformed);
        assert!(error && text.contains("unknown field"), "{text}");
        assert_eq!(index_hash(), before, "a malformed batch supplies no targeted hints");
    }
}
