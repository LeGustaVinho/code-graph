use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use codegraph_mcp::server::{tools_list, tools_list_for};
use serde_json::{json, Value};

fn rpc(id: u32, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params})
}

fn exchange(root: &Path, profile: Option<&str>, requests: &[Value]) -> Vec<String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codegraph"));
    command.args(["mcp", "--no-watch", "--root"]).arg(root);
    if let Some(profile) = profile {
        command.args(["--tool-profile", profile]);
    }
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut responses = Vec::new();
    for request in requests {
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        assert_ne!(output.read_line(&mut line).unwrap(), 0, "no response to {request}");
        responses.push(line);
    }
    drop(input);
    assert!(child.wait().unwrap().success());
    responses
}

fn result(response: &str) -> Value {
    serde_json::from_str::<Value>(response).unwrap()["result"].clone()
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[test]
fn compact_catalog_reuses_schemas_and_can_run_unlisted_tools() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
    let hidden = "get_file_skeleton";
    let calls = [
        rpc(1, "initialize", json!({"protocolVersion":"2025-11-25"})),
        rpc(2, "tools/list", json!({})),
        rpc(3, "tools/call", json!({"name":"get_tool_schema"})),
        rpc(4, "tools/call", json!({"name":"get_tool_schema","arguments":{"name":hidden}})),
        rpc(5, "tools/call", json!({"name":"get_tool_schema","arguments":{"name":"get_file_skel"}})),
        rpc(6, "tools/call", json!({"name":hidden,"arguments":{"file_path":"main.rs","include_private":true}})),
        rpc(
            7,
            "tools/call",
            json!({"name":"batch_query","arguments":{"operations":[{"tool":hidden,"args":{"file_path":"main.rs","include_private":true}}]}}),
        ),
    ];
    let default = exchange(root, None, &calls);
    let full = exchange(root, Some("full"), &calls);
    assert_eq!(default, full);
    assert_eq!(result(&full[1]), tools_list());
    assert_eq!(serde_json::from_str::<Value>(&full[2]).unwrap()["error"]["code"], -32602);

    let compact = exchange(root, Some("compact"), &calls);
    let init = result(&compact[0]);
    let instructions = init["instructions"].as_str().unwrap();
    assert!(instructions.contains("get_tool_schema") && instructions.contains("batch_query"));
    assert_eq!(init["capabilities"], result(&full[0])["capabilities"]);
    assert!(compact[..2].iter().map(String::len).sum::<usize>() < full[..2].iter().map(String::len).sum::<usize>());

    let listed = result(&compact[1]);
    let tools = listed["tools"].as_array().unwrap();
    let names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 12);
    assert!(names.contains(&"get_tool_schema") && names.contains(&"batch_query") && !names.contains(&hidden));
    for tool in tools.iter().filter(|t| t["name"] != "get_tool_schema") {
        let original = tools_list_for(tool["name"].as_str().unwrap()).unwrap();
        assert_eq!(*tool, original["tools"][0]);
    }

    let name_catalog = result(&compact[2]);
    assert_eq!(name_catalog["isError"], false);
    let catalog: Vec<_> = text(&name_catalog).lines().collect();
    let all = tools_list();
    assert!(all["tools"].as_array().unwrap().iter().all(|t| catalog.contains(&t["name"].as_str().unwrap())));
    let schema = result(&compact[3]);
    assert_eq!(schema["isError"], false);
    assert_eq!(serde_json::from_str::<Value>(text(&schema)).unwrap(), tools_list_for(hidden).unwrap());
    let unknown = result(&compact[4]);
    assert_eq!(unknown["isError"], true);
    assert!(text(&unknown).contains("unknown tool"));

    let direct = result(&full[5]);
    assert_eq!(direct["isError"], false);
    assert_eq!(direct, result(&compact[5]));
    let batch = result(&compact[6]);
    assert_eq!(batch["isError"], false);
    assert!(text(&batch).contains("### 1. get_file_skeleton") && text(&batch).contains(text(&direct)));
}
