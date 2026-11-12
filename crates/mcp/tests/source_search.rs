use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

#[test]
fn source_search_preserves_unicode_case_and_whole_word_matching() {
    let temp = tempfile::tempdir().unwrap();
    let source = "# Key KeySuffix\n# Keys\n# İ\n# İX\n# ΟΣ\n# οσ\n# prefix_key\n# key\nfn main() {}\n";
    std::fs::write(temp.path().join("main.rs"), source).unwrap();
    let engine = Engine::open(temp.path()).unwrap();

    for (query, case_sensitive, expected) in [
        ("key", true, vec!["main.rs:8 | # key"]),
        ("key", false, vec!["main.rs:1 | # Key KeySuffix", "main.rs:8 | # key"]),
        ("İ", false, vec!["main.rs:3 | # İ"]),
        ("ΟΣ", false, vec!["main.rs:5 | # ΟΣ"]),
    ] {
        let (text, error) = call_tool(
            &engine,
            "search_source",
            json!({
                "query": query, "case_sensitive": case_sensitive, "whole_word": true,
            }),
        );
        assert!(!error, "{text}");
        let header = format!(
            "{} matching line(s) in indexed source for '{query}' under . (1 indexed files scanned, 0 unreadable, 0 lossy UTF-8, 0 stale/error files excluded)",
            expected.len()
        );
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some(header.as_str()));
        assert_eq!(lines.collect::<Vec<_>>(), expected, "{text}");
    }
}
