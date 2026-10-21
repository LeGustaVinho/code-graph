//! MCP server: JSON-RPC 2.0 over stdio (newline-delimited), Tokio based.
//!
//! stdout carries protocol messages only; all logs go to stderr.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::engine::Engine;
use crate::tools;

pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "CodeGraph indexes this repository into a semantic graph (C#, TypeScript/JavaScript incl. Vue/Svelte components, Python, Rust, Prisma). \
Every relation carries a confidence level: exact (proven by imports/types/DI), inferred (naming, identical HTTP routes) or unresolved (ambiguous/dynamic). \
Results use exact `path:start-end` coordinates. Prefer get_file_skeleton / get_file_outline / get_symbol_body over reading whole files, \
use search_universal to locate symbols, and pass max_token_budget to bound any answer. batch_query runs several tools in one call.";

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn tools_list() -> Value {
    let tools: Vec<Value> = tools::registry()
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.schema)() }))
        .collect();
    json!({ "tools": tools })
}

/// Handles one JSON-RPC message. Returns `None` for notifications.
pub fn handle(engine: &Engine, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // Notifications (initialized, cancelled, ...) need no response.
        return None;
    };
    if msg.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
        return Some(error(&id, -32600, "invalid request: jsonrpc must be \"2.0\""));
    }
    let resp = match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(SUPPORTED_PROTOCOLS[0]);
            let version = if SUPPORTED_PROTOCOLS.contains(&requested) { requested } else { SUPPORTED_PROTOCOLS[0] };
            ok(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "codegraph", "title": "CodeGraph", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(&id, tools_list()),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(error(&id, -32602, "tools/call requires params.name"));
            };
            if !tools::registry().iter().any(|t| t.name == name) {
                return Some(error(&id, -32602, &format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
            let (text, is_error) = tools::call_tool(engine, name, args);
            ok(&id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        "resources/list" => ok(&id, json!({ "resources": [] })),
        "resources/templates/list" => ok(&id, json!({ "resourceTemplates": [] })),
        "prompts/list" => ok(&id, json!({ "prompts": [] })),
        "logging/setLevel" => ok(&id, json!({})),
        _ => error(&id, -32601, &format!("method not found: {method}")),
    };
    Some(resp)
}

/// Runs the stdio server until stdin closes.
pub async fn serve_stdio(engine: Arc<Engine>) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let engine = engine.clone();
        let tx = tx.clone();
        // Each request runs on the blocking pool; SQLite readers work concurrently in WAL mode.
        tokio::task::spawn_blocking(move || {
            let parsed: Result<Value, _> = serde_json::from_str(&line);
            let responses: Vec<Value> = match parsed {
                Err(e) => vec![error(&Value::Null, -32700, &format!("parse error: {e}"))],
                Ok(Value::Array(batch)) => batch.iter().filter_map(|m| handle(&engine, m)).collect(),
                Ok(msg) => handle(&engine, &msg).into_iter().collect(),
            };
            for r in responses {
                let _ = tx.send(r.to_string());
            }
        });
    }
    drop(tx);
    let _ = writer.await;
    Ok(())
}
