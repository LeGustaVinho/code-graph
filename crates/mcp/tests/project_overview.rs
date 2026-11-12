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

#[test]
fn scoped_overview_groups_relative_areas_and_excludes_other_apps() {
    let temp = tempfile::tempdir().unwrap();
    for (path, source) in [
        ("crates/desktop/build.rs", "fn main() {}\n"),
        ("crates/desktop/src/main.rs", "fn main() {}\n"),
        ("crates/desktop/src/commands/open.rs", "#[get(\"/desktop\")]\nfn open() {}\n"),
        ("crates/desktop/src/commands/deep/helper.rs", "pub fn helper() {}\n"),
        ("crates/desktop/assets/tools/info.rs", "pub fn info() {}\n"),
        ("crates/desktop/src/stale.rs", "#[get(\"/stale\")]\nfn stale() {}\n"),
        ("crates/desktop/vendor/tool/main.rs", "fn main() {}\n"),
        ("crates/desktop/.env", "DESKTOP_PORT=8080\n"),
        ("crates/desktop-extra/src/Other.cs", "public class Other {}\n"),
        ("crates/server/src/main.rs", "fn main() {}\n"),
        ("web/src/main.tsx", "export const App = () => <main />;\n"),
    ] {
        let file = temp.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let engine = Engine::open(temp.path()).unwrap();
    let conn = codegraph_graph::open(temp.path()).unwrap();
    conn.execute("UPDATE file_meta SET status = 'error' WHERE file_path = 'crates/desktop/src/stale.rs'", []).unwrap();

    let global = overview(&engine, json!({}));
    assert_eq!(overview(&engine, json!({ "path": "/" })), global);
    assert_eq!(overview(&engine, json!({ "path": "." })), global);
    assert!(global.contains("csharp=1") && global.contains("typescript=1"), "{global}");

    let scoped = overview(&engine, json!({ "path": ".\\crates//desktop\\" }));
    for evidence in [
        "Indexed scope crates/desktop | code files 5 | config 1 | vendor 1 | unindexed records 1",
        "Languages (code files): rust=5",
        "Code areas: 4 | Entrypoints: 3 (non-test)",
        "dir crates/desktop/src/commands/ (2 files)",
        "dir crates/desktop/ (1 files)",
        "dir crates/desktop/assets/tools/ (1 files)",
        "crates/desktop/src/main.rs:1",
        "crates/desktop/src/commands/open.rs:2",
    ] {
        assert!(scoped.contains(evidence), "missing {evidence} in:\n{scoped}");
    }
    for omitted in ["csharp", "typescript", "desktop-extra/", "crates/server/", "web/src/", "stale.rs", "vendor/tool/", "commands/deep/"] {
        assert!(!scoped.contains(omitted), "unexpected {omitted} in:\n{scoped}");
    }
    assert!(
        scoped.find("dir crates/desktop/assets/tools/").unwrap() < scoped.find("dir crates/desktop/src/ (1 files)").unwrap(),
        "{scoped}"
    );

    #[cfg(windows)]
    {
        let upper = overview(&engine, json!({ "path": "CRATES/DESKTOP" }));
        assert!(upper.lines().skip(1).eq(scoped.lines().skip(1)), "{upper}");
    }

    let exact = overview(&engine, json!({ "path": "crates/desktop/src/main.rs" }));
    assert!(exact.contains("code files 1 | config 0 | vendor 0") && exact.contains("Code areas: 0 | Entrypoints: 1"), "{exact}");
    assert!(exact.contains("crates/desktop/src/main.rs:1") && !exact.contains("  dir "), "{exact}");

    let absent = overview(&engine, json!({ "path": "crates/desktop/missing" }));
    assert!(
        absent.contains("No file records in the index match this scope.") && absent.contains("Code areas: 0 | Entrypoints: 0"),
        "{absent}"
    );
}
