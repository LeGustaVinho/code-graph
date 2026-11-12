use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, Engine};
use serde_json::{json, Value};

const FIRST_FILE: &str = "src/deep folder/feature/First.cs";
const SECOND_FILE: &str = "src/deep folder/feature/Second.cs";

fn search(engine: &Engine, args: Value) -> String {
    let (text, error) = call_tool(engine, "search_universal", args);
    assert!(!error, "{text}");
    text
}

fn results(text: &str) -> &str {
    // The first line includes elapsed time.
    text.split_once('\n').unwrap().1
}

#[test]
fn grouped_search_keeps_rank_and_metadata_under_result_and_token_limits() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src/deep folder/feature")).unwrap();
    std::fs::write(
        temp.path().join(FIRST_FILE),
        "namespace Demo;\nclass Alpha { public int Match() => 1; }\nclass Bravo { public int Match() {\nreturn 2;\n} }\n\
         class Delta { public int Match() => 3; }\nclass Echoo { public int Match() => 4; }\n",
    )
    .unwrap();
    std::fs::write(temp.path().join(SECOND_FILE), "namespace Demo;\nclass Chars { public int Match() => 5; }\n").unwrap();
    let engine = Engine::open(temp.path()).unwrap();

    let plain = search(&engine, json!({ "query": "Match", "kind": "method" }));
    let plain_hits: Vec<_> = results(&plain).lines().collect();
    assert_eq!(plain_hits.len(), 5, "{plain}");
    assert!(plain_hits[2].contains(SECOND_FILE), "{plain}");

    let grouped = search(&engine, json!({ "query": "Match", "kind": "method", "group_by_file": true }));
    let relative = |i: usize| format!("  {}", plain_hits[i].replacen(FIRST_FILE, "", 1));
    assert_eq!(results(&grouped), [plain_hits[0], &relative(1), plain_hits[2], plain_hits[3], &relative(4)].join("\n"),);
    let exact = search(&engine, json!({ "query": "Match", "kind": "method", "exact": true, "group_by_file": true }));
    assert_eq!(results(&exact), results(&grouped));

    let limited = search(&engine, json!({ "query": "Match", "kind": "method", "group_by_file": true, "limit": 1 }));
    assert_eq!(results(&limited).lines().next(), Some(plain_hits[0]));
    assert!(limited.contains("[MORE: showing 1 of 5 results"), "{limited}");

    let budget = estimate_tokens(grouped.lines().next().unwrap()) + estimate_tokens(plain_hits[0]) + 38;
    let capped = search(&engine, json!({ "query": "Match", "kind": "method", "group_by_file": true, "max_token_budget": budget }));
    assert!(estimate_tokens(&capped) <= budget, "{capped}");
    assert!(results(&capped).starts_with(&format!("{}\n[TRUNCATED: Showing 1 of 5 results", plain_hits[0])), "{capped}");

    let plain_json = search(&engine, json!({ "query": "Match", "format": "json" }));
    let grouped_json = search(&engine, json!({ "query": "Match", "format": "json", "group_by_file": true }));
    assert_eq!(results(&plain_json), results(&grouped_json));
    let first: Value = serde_json::from_str(results(&grouped_json).lines().next().unwrap()).unwrap();
    assert_eq!(first["qualified_name"], "Demo.Alpha.Match");
}
