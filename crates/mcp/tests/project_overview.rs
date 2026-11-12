use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

fn overview(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "get_project_overview", args);
    assert!(!error, "{text}");
    text
}

#[test]
fn overview_reports_coverage_and_diverse_entrypoint_evidence() {
    let temp = tempfile::tempdir().unwrap();
    for (path, source) in [
        ("main.rs", "fn main() {}\n"),
        ("server/core/routes.rs", "#[get(\"/first\")]\nfn first() {}\n#[get(\"/second\")]\nfn second() {}\n"),
        ("server/core/nested/support.rs", "pub fn helper() {}\n"),
        ("web/routes/api.rs", "#[get(\"/web\")]\nfn web() {}\n"),
        ("qa/Hooks.cs", "using NUnit.Framework;\npublic class Hooks {\n    [SetUp]\n    public void Prepare() {}\n}\n"),
        ("vendor/tool/main.rs", "fn main() {}\n"),
        (".env", "EXAMPLE_PORT=8080\n"),
    ] {
        let file = temp.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let engine = Engine::open(temp.path()).unwrap();
    // Check that excluded vendor/test entries exist in the underlying graph.
    let (all_entries, error) = call_tool(&engine, "get_entrypoints", json!({}));
    assert!(!error && all_entries.contains("vendor/tool/main.rs") && all_entries.contains("test hook"), "{all_entries}");

    let text = overview(&engine, json!({ "limit": 3 }));
    for evidence in [
        "code files 5 | config 1 | vendor 1 | unindexed records 0",
        "Languages (code files): rust=4 csharp=1",
        "Code areas: 4 | Entrypoints: 4 (non-test)",
        "dir server/core/ (2 files)",
        "dir ./ (1 files)",
        "server/core/routes.rs:2",
        "main.rs:1",
        "web/routes/api.rs:2",
        "[MORE:",
    ] {
        assert!(text.contains(evidence), "missing {evidence} in:\n{text}");
    }
    for omitted in ["vendor/tool/main.rs", "test hook", "route GET /second"] {
        assert!(!text.contains(omitted), "unexpected {omitted} in:\n{text}");
    }

    let budget = 130;
    let bounded = overview(&engine, json!({ "max_token_budget": budget }));
    assert!(estimate_tokens(&bounded) <= budget, "{bounded}");
    assert!(bounded.contains("dir server/core/") && bounded.contains("main.rs:1"), "{bounded}");
}

#[test]
fn overview_reports_empty_index() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::open(temp.path()).unwrap();
    let text = overview(&engine, json!({}));
    assert!(text.contains("code files 0") && text.contains("Code areas: 0 | Entrypoints: 0"), "{text}");
}
