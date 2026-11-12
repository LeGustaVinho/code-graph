use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

fn find(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "find_files", args);
    assert!(!error, "{text}");
    text
}

#[test]
fn file_search_ranks_literal_suffixes_and_respects_scope_and_budgets() {
    let temp = tempfile::tempdir().unwrap();
    for path in [
        "main.ts",
        "src/renderer/main.ts",
        "src/renderer/main.tsx",
        "src/renderer/local-main.ts",
        "src/renderer/nested/main.ts",
        "src/notrenderer/main.ts",
        "src/renderer2/main.ts",
        "src/renderer/odd%_name.ts",
        "src/renderer/oddXXname.ts",
    ] {
        let file = temp.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "export const value = 1;\n").unwrap();
    }
    let engine = Engine::open(temp.path()).unwrap();
    let result = find(&engine, json!({ "query": "renderer\\main.ts" }));
    let expected = ["src/renderer/main.ts", "src/renderer/main.tsx", "src/renderer/local-main.ts"];
    assert!(result.starts_with("3 matching indexed path(s) under . (9 indexed files, 0 stale/error excluded)"), "{result}");
    assert_eq!(result.lines().skip(1).collect::<Vec<_>>(), expected);
    let scoped = find(&engine, json!({ "query": "main.ts", "path": ".\\src//renderer\\" }));
    assert_eq!(
        scoped.lines().skip(1).collect::<Vec<_>>(),
        ["src/renderer/main.ts", "src/renderer/nested/main.ts", "src/renderer/main.tsx", "src/renderer/local-main.ts"]
    );
    let literal = find(&engine, json!({ "query": "%_" }));
    assert_eq!(literal.lines().skip(1).collect::<Vec<_>>(), ["src/renderer/odd%_name.ts"]);

    let limited = find(&engine, json!({ "query": "renderer/main.ts", "limit": 1 }));
    assert_eq!(limited.lines().nth(1), Some(expected[0]));
    assert!(limited.contains("[MORE: showing 1 of 3 results"), "{limited}");

    let budget = estimate_tokens(result.lines().next().unwrap()) + estimate_tokens(expected[0]) + 34;
    let capped = find(&engine, json!({ "query": "renderer/main.ts", "max_token_budget": budget }));
    assert!(estimate_tokens(&capped) <= budget, "{capped}");
    assert_eq!(capped.lines().nth(1), Some(expected[0]));
    assert!(capped.contains("[TRUNCATED: Showing 1 of 3 results"), "{capped}");

    let at_root = call_tool(&engine, "search_source", json!({ "query": "value", "limit": 1 }));
    let legacy_root = call_tool(&engine, "search_source", json!({ "query": "value", "path": "/", "limit": 1 }));
    assert!(!at_root.1);
    assert_eq!(legacy_root, at_root);
}

#[test]
fn file_search_distinguishes_index_coverage_from_disk_presence() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("stale.ts"), "export const value = 1;\n").unwrap();
    std::fs::write(temp.path().join("stale.txt"), "not indexed\n").unwrap();
    let engine = Engine::open(temp.path()).unwrap();
    let writer = codegraph_graph::open(temp.path()).unwrap();
    assert_eq!(writer.execute("UPDATE file_meta SET status = 'stale' WHERE file_path = 'stale.ts'", []).unwrap(), 1);

    let result = find(&engine, json!({ "query": "stale" }));
    assert!(result.starts_with("0 matching indexed path(s) under . (0 indexed files, 1 stale/error excluded)"), "{result}");
    assert!(result.contains("No match in the index. Ignored, unsupported and other unindexed files were not searched."), "{result}");

    for args in [
        json!({ "query": "" }),
        json!({ "query": "src/" }),
        json!({ "query": "stale", "path": "../" }),
        json!({ "query": "stale", "path": "/" }),
    ] {
        let (text, error) = call_tool(&engine, "find_files", args);
        assert!(error && text.starts_with("invalid arguments:"), "{text}");
    }
}
