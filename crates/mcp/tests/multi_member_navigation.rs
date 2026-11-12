//! Selecting multiple names should remove repeated discovery round trips
//! without silently widening the answer to unrelated declarations.

use codegraph_mcp::{call_tool, Engine};
use serde_json::json;

fn ok(e: &Engine, tool: &str, args: serde_json::Value) -> String {
    let (text, failed) = call_tool(e, tool, args);
    assert!(!failed, "{tool}: {text}");
    text
}

#[test]
fn selective_outline_retains_ancestors_and_exact_names() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Controller.cs"),
        "public class Controller {\n  private void Start() { Load(); }\n  private void Stop() { Save(); }\n  public void StartAsync() { }\n  private void Load() { }\n  private void Save() { }\n}\n",
    ).unwrap();
    let e = Engine::open(temp.path()).unwrap();
    let outline = ok(&e, "get_file_outline", json!({
        "file_path": "Controller.cs",
        "names": ["Start", "Stop"], "include_signatures": true
    }));
    assert!(outline.contains("class Controller"), "{outline}");
    assert!(outline.contains("Start") && outline.contains("Stop"), "{outline}");
    assert!(!outline.contains("StartAsync"), "{outline}");
    assert!(!outline.contains("Load") && !outline.contains("Save"), "{outline}");
    assert!(outline.contains("Start(): void") && outline.contains("Stop(): void"), "{outline}");
    let (_, failed) = call_tool(&e, "get_file_outline", json!({
        "file_path": "Controller.cs", "filter": "Start", "names": ["Stop"]
    }));
    assert!(failed, "ambiguous selector should fail");
}

#[test]
fn selective_flow_works_for_multiple_languages_and_names() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Worker.cs"),
        "class Worker {\n  void Begin() { if (ready) { StartAsync(); } }\n  void End() { if (done) { return; } }\n  void BeginAsync() { Log(); }\n}\n",
    ).unwrap();
    std::fs::write(
        temp.path().join("tasks.py"),
        "class Tasks:\n    def begin(self):\n        if True:\n            return 1\n    def end(self):\n        raise ValueError('done')\n    def beginning(self):\n        return 2\n",
    ).unwrap();
    std::fs::write(
        temp.path().join("handler.ts"),
        "export class Handler {\n  begin() { if (true) return 'a'; }\n  end() { return 'b'; }\n  beginning() { return 'c'; }\n}\n",
    ).unwrap();
    std::fs::write(
        temp.path().join("handler.rs"),
        "pub struct Handler;\nimpl Handler {\n  pub fn begin(&self) { println!(\"start\"); }\n  pub fn end(&self) { println!(\"stop\"); }\n  pub fn beginning(&self) { println!(\"skip\"); }\n}\n",
    ).unwrap();
    let e = Engine::open(temp.path()).unwrap();
    let csharp = ok(&e, "get_flow_evidence", json!({
        "symbol_id": "Worker", "members": ["Begin", "End"], "max_lines": 25,
    }));
    assert!(csharp.contains("Worker.Begin") && csharp.contains("Worker.End"), "{csharp}");
    assert!(!csharp.contains("BeginAsync"), "{csharp}");
    let py = ok(&e, "get_flow_evidence", json!({
        "symbol_id": "Tasks", "members": ["begin", "end"]
    }));
    assert!(py.contains("def begin") && py.contains("def end"), "{py}");
    assert!(!py.contains("def beginning"), "{py}");
    let ts = ok(&e, "get_flow_evidence", json!({
        "symbol_id": "handler.ts:Handler", "members": ["begin", "end"]
    }));
    assert!(ts.contains("begin") && ts.contains("end"), "{ts}");
    assert!(!ts.contains("beginning"), "{ts}");
    let rust = ok(&e, "get_file_outline", json!({
        "file_path": "handler.rs", "names": ["begin", "end"], "include_signatures": true
    }));
    assert!(rust.contains("begin") && rust.contains("end"), "{rust}");
    assert!(!rust.contains("beginning"), "{rust}");
    let (_, failed) = call_tool(&e, "get_flow_evidence", json!({
        "symbol_id": "Worker", "filter": "Be", "members": ["End"]
    }));
    assert!(failed);
}
