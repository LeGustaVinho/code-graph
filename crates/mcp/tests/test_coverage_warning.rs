//! A missing indexed test edge is weaker than proof that a test does not exist.

use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

#[test]
fn test_lookup_with_no_indexed_tests_warns_about_incomplete_coverage() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Plain.cs"), "class Plain { public void Run() {} }").unwrap();
    let engine = Engine::open(tmp.path()).unwrap();
    let (text, failed) = call_tool(&engine, "get_tests_for_symbol", json!({"symbol_id": "Plain.Run"}));
    assert!(!failed, "{text}");
    assert!(text.contains("0 linked test(s)"), "{text}");
    assert!(text.contains("absence of tests is unverified"), "{text}");
}
