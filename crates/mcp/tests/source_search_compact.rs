use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

fn search(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "search_source", args);
    assert!(!error, "{text}");
    text
}

#[test]
fn group_by_file_retains_line_order_limits_and_json() {
    let temp = tempfile::tempdir().unwrap();
    let dir = "src/deep feature";
    std::fs::create_dir_all(temp.path().join(dir)).unwrap();
    for file in ["first.rs", "second.rs"] {
        std::fs::write(temp.path().join(dir).join(file), "// target first\r\n// elsewhere\r\n// target last\r\n").unwrap();
    }
    let engine = Engine::open(temp.path()).unwrap();
    let plain = search(&engine, json!({ "query": "target" }));
    let grouped = search(&engine, json!({ "query": "target", "group_by_file": true }));
    let header = plain.lines().next().unwrap();
    let expected = [
        header,
        "src/deep feature/first.rs:1 | // target first",
        "  :3 | // target last",
        "src/deep feature/second.rs:1 | // target first",
        "  :3 | // target last",
    ];
    assert_eq!(grouped, expected.join("\n"));
    let limited = search(&engine, json!({ "query": "target", "group_by_file": true, "limit": 2 }));
    assert!(limited.contains("[MORE: showing 2 of 4 results"), "{limited}");
    assert_eq!(limited.lines().take(3).collect::<Vec<_>>(), expected[..3]);

    let budget = estimate_tokens(header) + estimate_tokens(expected[1]) + 34;
    let capped = search(&engine, json!({ "query": "target", "group_by_file": true, "max_token_budget": budget }));
    assert!(estimate_tokens(&capped) <= budget, "{capped}");
    assert_eq!(capped.lines().take(2).collect::<Vec<_>>(), expected[..2]);
    assert!(capped.contains("[TRUNCATED: Showing 1 of 4 results"), "{capped}");

    let plain_json = search(&engine, json!({ "query": "target", "format": "json" }));
    let grouped_json = search(&engine, json!({ "query": "target", "format": "json", "group_by_file": true }));
    assert_eq!(grouped_json, plain_json);
}

#[test]
fn long_excerpts_include_the_selected_match_and_keep_source_spelling() {
    let temp = tempfile::tempdir().unwrap();
    let long_needle = "LongToken".repeat(24);
    let source = format!(
        "　// needleSuffix {} needle {}\n// {}{} Token Key {}\n// {}İ tail\n// {} {long_needle} end\n// early {}\n// {} ",
        "a".repeat(350),
        "b".repeat(200),
        "İ".repeat(340),
        "K".repeat(22),
        "🦀".repeat(100),
        "K".repeat(340),
        "c".repeat(220),
        "x".repeat(200),
        "d".repeat(320),
    );
    let mut bytes = source.into_bytes();
    bytes.extend_from_slice(b"\xff lossy-target tail\n");
    std::fs::write(temp.path().join("source.rs"), &bytes).unwrap();
    let engine = Engine::open(temp.path()).unwrap();

    for (query, case_sensitive, whole_word, line, evidence) in [
        ("needle", true, true, 1, " needle "),
        ("token key", false, true, 2, "Token Key"),
        ("\u{0307} tail", false, false, 3, "İ tail"),
        (long_needle.as_str(), true, true, 4, "…LongTokenLongToken"),
        ("early", true, true, 5, "// early"),
        ("lossy-target", true, true, 6, "� lossy-target"),
    ] {
        let response = search(&engine, json!({ "query": query, "case_sensitive": case_sensitive, "whole_word": whole_word }));
        assert!(response.starts_with("1 matching line(s)") && response.contains("1 lossy UTF-8"), "{response}");
        let hit = response.lines().nth(1).unwrap();
        let excerpt = hit.strip_prefix(&format!("source.rs:{line} | ")).unwrap();
        assert!(excerpt.contains(evidence) && excerpt.chars().count() <= 180, "{excerpt}");
        if query != "early" {
            assert!(excerpt.starts_with('…'), "{excerpt}");
        }

        let full = search(&engine, json!({ "query": query, "case_sensitive": case_sensitive, "whole_word": whole_word, "format": "json" }));
        let record: Value = serde_json::from_str(full.lines().nth(1).unwrap()).unwrap();
        let original = String::from_utf8_lossy(&bytes);
        assert_eq!(record, json!({ "file": "source.rs", "line": line, "text": original.lines().nth(line - 1).unwrap().trim() }));
    }
}
