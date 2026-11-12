use std::process::{Command, Output};

use serde_json::{json, Value};

fn tools(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codegraph")).arg("tools").args(args).output().unwrap()
}

fn compact_json(output: Output) -> Value {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(text.lines().count(), 1, "expected compact JSON");
    serde_json::from_str(&text).unwrap()
}

#[test]
fn single_tool_schema_matches_the_full_json_list() {
    let all = compact_json(tools(&["--json"]));
    assert_eq!(all, codegraph_mcp::server::tools_list());

    let expected = all["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "search_universal").unwrap();
    for args in [["search_universal", "--json"], ["--json", "search_universal"]] {
        let selected = compact_json(tools(&args));
        assert_eq!(selected, json!({ "tools": [expected] }));
    }
}

#[test]
fn unknown_tool_fails_without_json_on_stdout() {
    let output = tools(&["search_univer", "--json"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unknown tool 'search_univer'") && stderr.contains("codegraph tools"), "{stderr}");
}
