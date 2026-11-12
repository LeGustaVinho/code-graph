use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

fn search(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "search_source", args);
    assert!(!error, "{text}");
    text
}

#[test]
fn multi_search_preserves_each_terms_evidence_and_reports_counts_once() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("src")).unwrap();
    let long = format!("// Token {} Key\n", "x".repeat(350));
    std::fs::write(root.join("src/a.rs"), format!("// Token Key\n{long}// Keys, not keySuffix\n")).unwrap();
    std::fs::write(root.join("src/b.rs"), "// key\n").unwrap();
    std::fs::write(root.join("outside.rs"), "// key and token outside\n").unwrap();
    let engine = Engine::open(root).unwrap();

    let args = json!({
        "queries": [" key ", "token", "missing"], "path": "src", "limit": 2,
        "case_sensitive": false, "whole_word": true, "group_by_file": true,
    });
    let multi = search(&engine, args.clone());
    let lines: Vec<_> = multi.lines().collect();
    assert_eq!(lines[0], "3 source queries under src (2 indexed files scanned, 0 unreadable, 0 lossy UTF-8, 0 stale/error files excluded)");
    assert!(lines[1].starts_with("Matching lines by query order: [3, 2, 0]. Limit 2 per query."), "{multi}");
    assert!(lines[1].contains("1 hit(s) over the per-query limit."), "{multi}");
    assert_eq!((lines[2], lines[5], lines[8]), ("1. \"key\"", "2. \"token\"", "3. \"missing\" (0 matches)"));
    for (needle, group) in [("key", &lines[3..5]), ("token", &lines[6..8])] {
        let single = search(
            &engine,
            json!({
                "query": needle, "path": "src", "limit": 2,
                "case_sensitive": false, "whole_word": true, "group_by_file": true,
            }),
        );
        assert_eq!(single.lines().skip(1).take(2).collect::<Vec<_>>(), group);
    }
    assert!(lines[4].contains("Key") && lines[7].contains("Token"), "{multi}");
    assert!(!multi.contains("outside.rs"), "{multi}");

    let mut root_args = args.clone();
    root_args.as_object_mut().unwrap().remove("path");
    assert!(search(&engine, root_args).contains("Matching lines by query order: [4, 3, 0]"));

    let first_item = lines[2..4].join("\n");
    let budget = lines[..2].iter().map(|line| estimate_tokens(line) + 1).sum::<usize>() + estimate_tokens(&first_item) + 33;
    let mut capped_args = args;
    capped_args["max_token_budget"] = json!(budget);
    let capped = search(&engine, capped_args);
    assert!(estimate_tokens(&capped) <= budget && capped.contains("[TRUNCATED: Showing 1 of 5 results"), "{capped}");
    assert_eq!(capped.lines().take(4).collect::<Vec<_>>(), lines[..4]);
    assert!(!capped.contains("2. \"token\""), "{capped}");
}

#[test]
fn multi_search_validates_arguments_and_shared_empty_scope() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::create_dir(root.join("ignored")).unwrap();
    std::fs::write(root.join(".codegraphignore"), "ignored/\n").unwrap();
    std::fs::write(root.join("ignored/hidden.rs"), "fn hidden() {}\n").unwrap();
    let engine = Engine::open(root).unwrap();

    let verified = search(&engine, json!({ "queries": ["main", "other"], "path": "empty" }));
    assert!(verified.contains("All 2 queries have 0 matching lines") && verified.contains("verified empty source scope"), "{verified}");

    for (args, expected) in [
        (json!({}), "provide exactly one"),
        (json!({ "query": "main", "queries": ["other"] }), "provide exactly one"),
        (json!({ "queries": [] }), "1–8 search texts"),
        (json!({ "queries": vec!["main"; 9] }), "1–8 search texts"),
        (json!({ "queries": ["main", "  "] }), "queries[1] must not be empty"),
        (json!({ "queries": ["main"], "format": "json" }), "format=json requires a single query"),
        (json!({ "queries": ["main", "hidden"], "path": "ignored" }), "absence cannot be established"),
        (json!({ "queries": ["main", "hidden"], "path": "missing" }), "does not exist relative to the project root"),
    ] {
        let (text, error) = call_tool(&engine, "search_source", args);
        assert!(error && text.contains(expected), "expected {expected:?}, got {text}");
    }
}
