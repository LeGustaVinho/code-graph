//! MCP server: JSON-RPC 2.0 over stdio (newline-delimited), Tokio based.
//!
//! stdout carries protocol messages only; all logs go to stderr.

use std::sync::Arc;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::engine::Engine;
use crate::tools;

pub const SUPPORTED_PROTOCOLS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "CodeGraph indexes this repository into a semantic graph (C#, TypeScript/JavaScript incl. Vue/Svelte components, Python, Rust, Prisma). \
Every relation carries a confidence level: exact (proven by imports/types/DI), inferred (naming, identical HTTP routes) or unresolved (ambiguous/dynamic). \
Start unfamiliar projects with get_project_overview. Results use path and line coordinates. Prefer get_file_skeleton / get_file_outline / get_symbol_body over reading whole files, \
use search_universal to locate symbols, and pass max_token_budget to bound any answer. batch_query runs several tools in one call.";

const COMPACT_INSTRUCTIONS: &str =
    "Start with get_project_overview. Use find_files, search_source, search_universal and get_symbol_body for file:line evidence. \
Search covers indexed, readable source. Relations are exact (proven), inferred or unresolved. \
Use get_tool_schema to list tool names or get one exact schema, then batch_query to run tools outside tools/list.";

const CATALOG_TOOL: &str = "get_tool_schema";

const COMPACT_TOOLS: &[&str] = &[
    "batch_query",
    "find_files",
    "get_call_hierarchy",
    "get_file_outline",
    "get_flow_evidence",
    "get_project_overview",
    "get_symbol_body",
    "get_tests_for_symbol",
    "search_source",
    "search_universal",
    "trace_fullstack_flow",
];

/// Controls which tool definitions this MCP process advertises.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolProfile {
    #[default]
    Full,
    Compact,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CatalogParams {
    /// Exact tool name. Omit to list tool names.
    name: Option<String>,
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn tools_list() -> Value {
    tool_definitions(tools::registry(), None)
}

/// Returns the same tool list envelope with a single exact name.
pub fn tools_list_for(name: &str) -> Option<Value> {
    let tool = tools::registry().into_iter().find(|tool| tool.name == name)?;
    Some(tool_definitions([tool], None))
}

fn tool_definitions(definitions: impl IntoIterator<Item = tools::ToolDef>, extra: Option<Value>) -> Value {
    let tools: Vec<Value> = definitions
        .into_iter()
        .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": (t.schema)() }))
        .chain(extra)
        .collect();
    json!({ "tools": tools })
}

fn catalog_tool_definition() -> Value {
    json!({
        "name": CATALOG_TOOL,
        "description": "List CodeGraph tool names or show the schema for one exact name. Run unlisted tools with batch_query.",
        "inputSchema": tools::schema_of::<CatalogParams>(),
    })
}

fn compact_tools_list() -> Value {
    tool_definitions(tools::registry().into_iter().filter(|t| COMPACT_TOOLS.contains(&t.name)), Some(catalog_tool_definition()))
}

fn catalog_tool(args: Value) -> (String, bool) {
    let params: CatalogParams = match tools::parse_args(args) {
        Ok(params) => params,
        Err(err) => return (err.to_string(), true),
    };
    match params.name.as_deref() {
        Some(CATALOG_TOOL) => (json!({ "tools": [catalog_tool_definition()] }).to_string(), false),
        Some(name) => match tools_list_for(name) {
            Some(schema) => (schema.to_string(), false),
            None => (format!("unknown tool '{name}'. Omit name to list CodeGraph tool names."), true),
        },
        None => {
            let mut names: Vec<_> = tools::registry().into_iter().map(|t| t.name).chain([CATALOG_TOOL]).collect();
            names.sort_unstable();
            (names.join("\n"), false)
        }
    }
}

/// Handles one JSON-RPC message. Returns `None` for notifications.
pub fn handle(engine: &Engine, msg: &Value) -> Option<Value> {
    handle_with_profile(engine, msg, ToolProfile::Full)
}

/// Handles one JSON-RPC message with the process's tool profile.
pub fn handle_with_profile(engine: &Engine, msg: &Value, profile: ToolProfile) -> Option<Value> {
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
                    "instructions": match profile {
                        ToolProfile::Full => INSTRUCTIONS,
                        ToolProfile::Compact => COMPACT_INSTRUCTIONS,
                    },
                }),
            )
        }
        "ping" => ok(&id, json!({})),
        "tools/list" => ok(
            &id,
            match profile {
                ToolProfile::Full => tools_list(),
                ToolProfile::Compact => compact_tools_list(),
            },
        ),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(error(&id, -32602, "tools/call requires params.name"));
            };
            let catalog_call = profile == ToolProfile::Compact && name == CATALOG_TOOL;
            if !catalog_call && !tools::registry().iter().any(|t| t.name == name) {
                return Some(error(&id, -32602, &format!("unknown tool: {name}")));
            }
            let args = params.get("arguments").cloned().unwrap_or(Value::Object(Default::default()));
            let (text, is_error) = if catalog_call { catalog_tool(args) } else { tools::call_tool(engine, name, args) };
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
    serve_stdio_with_profile(engine, ToolProfile::Full).await
}

/// Runs a stdio server with one stable tool profile until stdin closes.
pub async fn serve_stdio_with_profile(engine: Arc<Engine>, profile: ToolProfile) -> Result<()> {
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
                Ok(Value::Array(batch)) => batch.iter().filter_map(|m| handle_with_profile(&engine, m, profile)).collect(),
                Ok(msg) => handle_with_profile(&engine, &msg, profile).into_iter().collect(),
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
