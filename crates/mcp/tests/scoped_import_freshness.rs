use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

#[test]
fn scoped_alias_refreshes_importer_and_target_without_watcher() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let importer = root.join("consumer.ts");
    let target = root.join("target.ts");
    std::fs::write(&importer, "export const unused = 1;\n").unwrap();
    std::fs::write(&target, "export function actual() { return 1; }\n").unwrap();
    let engine = Engine::open(root).unwrap();
    let args = json!({ "symbol_id": "consumer.ts:Alias/0" });

    std::fs::write(&importer, "import { actual as Alias } from './target';\nexport const value = Alias();\n").unwrap();
    let (first, error) = call_tool(&engine, "get_symbol_body", args.clone());
    assert!(!error && first.contains("actual target.ts:1") && first.contains("return 1;"), "{first}");

    std::fs::write(&target, "// move the declaration\n\nexport function actual() { return 42; }\n").unwrap();
    let (latest, error) = call_tool(&engine, "get_symbol_body", args);
    assert!(!error && latest.contains("actual target.ts:3") && latest.contains("return 42;"), "{latest}");

    let (search, error) = call_tool(&engine, "search_universal", json!({ "query": "Alias", "exact": true }));
    assert!(!error && search.starts_with("0 exact declaration match(es)"), "{search}");
}

#[test]
fn scoped_alias_refreshes_a_known_module_when_its_export_appears() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let target = root.join("target.ts");
    std::fs::write(root.join("consumer.ts"), "import { actual as Alias } from './target';\n").unwrap();
    std::fs::write(&target, "export const unused = 1;\n").unwrap();
    let engine = Engine::open(root).unwrap();
    let args = json!({ "symbol_id": "consumer.ts:Alias" });

    let (missing, error) = call_tool(&engine, "get_symbol_body", args.clone());
    assert!(error && missing.contains("No symbol matches"), "{missing}");

    std::fs::write(&target, "// new export\nexport function actual() { return 42; }\n").unwrap();
    let (latest, error) = call_tool(&engine, "get_symbol_body", args);
    assert!(!error && latest.contains("actual target.ts:2") && latest.contains("return 42;"), "{latest}");
}

#[test]
fn scoped_alias_refreshes_the_new_target_after_the_import_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let importer = root.join("consumer.ts");
    let target = root.join("new.ts");
    std::fs::write(&importer, "import { actual as Alias } from './old';\n").unwrap();
    std::fs::write(root.join("old.ts"), "export function actual() { return 1; }\n").unwrap();
    std::fs::write(&target, "export function actual() { return 2; }\n").unwrap();
    let engine = Engine::open(root).unwrap();
    let args = json!({ "symbol_id": "consumer.ts:Alias" });
    let (before, error) = call_tool(&engine, "get_symbol_body", args.clone());
    assert!(!error && before.contains("actual old.ts:1"), "{before}");

    std::fs::write(&importer, "// moved import\nimport { actual as Alias } from './new';\n").unwrap();
    std::fs::write(&target, "// moved declaration\n\nexport function actual() { return 42; }\n").unwrap();
    let (after, error) = call_tool(&engine, "get_symbol_body", args);
    assert!(!error && after.contains("actual new.ts:3") && after.contains("return 42;"), "{after}");
}
