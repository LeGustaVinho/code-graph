//! MCP tool tests: token budgets, freshness, protocol conformance and a smoke run of all 28 tools.

use std::path::{Path, PathBuf};
use std::time::Instant;

use codegraph_core::tokens::estimate_tokens;
use codegraph_mcp::{call_tool, server, Engine};
use serde_json::{json, Value};

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for e in std::fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if e.file_name() == ".codegraph" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &dst.join(e.file_name()));
        } else {
            std::fs::copy(&p, dst.join(e.file_name())).unwrap();
        }
    }
}

fn engine() -> (tempfile::TempDir, std::sync::Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fullstack"), &root);
    let e = Engine::open(&root).unwrap();
    (tmp, e)
}

fn ok(engine: &Engine, tool: &str, args: Value) -> String {
    let (text, is_error) = call_tool(engine, tool, args.clone());
    assert!(!is_error, "{tool} {args} failed: {text}");
    text
}

#[test]
fn token_budget_is_strictly_enforced() {
    let (_t, e) = engine();
    for (tool, args) in [
        ("search_universal", json!({ "query": "o", "limit": 100, "max_token_budget": 100 })),
        ("get_entrypoints", json!({ "max_token_budget": 100 })),
        ("find_literal", json!({ "query": "a", "max_token_budget": 100 })),
        ("get_call_hierarchy", json!({ "symbol_id": "Dispatcher", "max_token_budget": 100 })),
    ] {
        let text = ok(&e, tool, args);
        assert!(estimate_tokens(&text) <= 100, "{tool}: {} tokens\n{text}", estimate_tokens(&text));
        if tool != "get_call_hierarchy" {
            assert!(text.contains("[TRUNCATED: Showing "), "{tool} should be truncated:\n{text}");
            assert!(text.contains("token budget ("), "{text}");
        }
    }
}

#[test]
fn freshness_check_reflects_edits_before_answering() {
    let (_t, e) = engine();
    let before = ok(&e, "get_symbol_body", json!({ "symbol_id": "OrderService.CancelOrderAsync", "max_lines": 1 }));
    assert!(before.contains("OrderService.cs:52-62"), "{before}");
    // Insert three lines above the method; no watcher, no manual re-index.
    let f = e.root.join("backend-dotnet/Services/OrderService.cs");
    let src = std::fs::read_to_string(&f).unwrap();
    let edited = src.replacen(
        "        public async Task CancelOrderAsync(Guid id, string reason)",
        "        public int Pending() => _placedCount;\n\n\n        public async Task CancelOrderAsync(Guid id, string reason)",
        1,
    );
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&f, edited).unwrap();
    let after = ok(&e, "get_symbol_body", json!({ "symbol_id": "OrderService.CancelOrderAsync", "max_lines": 1 }));
    assert!(after.contains("OrderService.cs:55-65"), "new line range expected:\n{after}");
    assert!(after.contains("  55| "), "{after}");
    let outline = ok(&e, "get_file_outline", json!({ "file_path": "OrderService.cs" }));
    assert!(outline.contains("m Pending :52"), "{outline}");
}

#[test]
fn mcp_protocol_messages() {
    let (_t, e) = engine();
    let init = server::handle(
        &e,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {} } }),
    )
    .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    assert!(server::handle(&e, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none());
    let list = server::handle(&e, &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 28);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
        assert!(t["description"].as_str().unwrap().len() > 20);
    }
    let call = server::handle(&e, &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "get_file_skeleton", "arguments": { "file_path": "OrderService.cs" } } })).unwrap();
    assert_eq!(call["result"]["isError"], false);
    assert!(call["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("PlaceOrderAsync(Order order, int priority): Task<Order> :26-50"));
    // Strict argument validation.
    let bad = server::handle(
        &e,
        &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "search_universal", "arguments": { "q": "x" } } }),
    )
    .unwrap();
    assert_eq!(bad["result"]["isError"], true);
    let unknown = server::handle(&e, &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "nope" } })).unwrap();
    assert_eq!(unknown["error"]["code"], -32602);
    let missing = server::handle(&e, &json!({ "jsonrpc": "2.0", "id": 6, "method": "does/not/exist" })).unwrap();
    assert_eq!(missing["error"]["code"], -32601);
}

#[test]
fn all_28_tools_answer() {
    let (_t, e) = engine();
    let calls: Vec<(&str, Value, &str)> = vec![
        (
            "batch_query",
            json!({ "operations": [{ "tool": "get_file_outline", "args": { "file_path": "auth.ts" } }, { "tool": "get_entrypoints" }] }),
            "### 2. get_entrypoints",
        ),
        ("get_file_skeleton", json!({ "file_path": "backend-python/app/services.py" }), "class ItemService:"),
        (
            "simulate_signature_change",
            json!({ "symbol_id": "OrderService.PlaceOrderAsync", "new_param_count": 3 }),
            "BREAKS backend-dotnet/Controllers/OrdersController.cs:46",
        ),
        ("get_flow_diagram", json!({ "symbols": ["OrderList", "OrderService.CancelOrderAsync"] }), "flowchart LR"),
        ("search_universal", json!({ "query": "ItemServ" }), "class ItemService"),
        ("find_literal", json!({ "query": "/api/orders", "exact": true }), "frontend-react/src/api/orders.ts:11"),
        ("find_variable_or_const", json!({ "name": "CACHE" }), "W backend-python/app/services.py:20"),
        ("trace_execution_path", json!({ "source": "OrderList", "target": "OrderService.CancelOrderAsync" }), "3 hop(s)"),
        ("trace_fullstack_flow", json!({ "api_route_or_call": "GET /api/orders" }), "ROUTE GET /api/Orders"),
        ("resolve_dependency_injection", json!({ "interface_or_token": "IClock" }), "IClock → SystemClock"),
        ("get_call_hierarchy", json!({ "symbol_id": "OrderService.CancelOrderAsync", "direction": "callers" }), "OrdersController.Cancel"),
        ("get_type_hierarchy", json!({ "symbol_id": "OrderService" }), "implements iface IOrderService"),
        ("get_impact_radius", json!({ "symbol_id": "hash" }), "AuthStore.login"),
        ("get_state_mutations", json!({ "symbol_id": "OrderService.PlaceOrderAsync" }), "database Orders"),
        ("trace_error_flow", json!({ "exception_name_or_message": "ItemNotFound" }), "CAUGHT by fn handle_not_found"),
        ("find_by_capability", json!({ "tags": ["NETWORK_IO"] }), "getOrders"),
        ("get_entrypoints", json!({ "kind": "route" }), "route POST /api/Orders/{id}/cancel"),
        ("find_orphaned_symbols", json!({}), "unusedHelper"),
        ("get_data_model", json!({ "model_name": "Invoice" }), "FK customerId: String → Customer (N:1)"),
        ("get_config_matrix", json!({}), "SHOP_REGION"),
        ("get_type_contract", json!({ "type_id": "PaymentGateway" }), "charge(amount: number): Promise<boolean>"),
        ("get_symbol_body", json!({ "symbol_id": "get_db" }), "  15|"),
        ("get_file_outline", json!({ "file_path": "OrderList.tsx" }), "fn OrderList"),
        ("get_complexity_hotspots", json!({ "min_complexity": 3 }), "cc7"),
        ("validate_dependency_rule", json!({ "source_module": "domain", "target_module": "infrastructure" }), "VIOLATION"),
        ("get_tests_for_symbol", json!({ "symbol_id": "AuthStore.login" }), "returns a token on login"),
    ];
    for (tool, args, expect) in &calls {
        let text = ok(&e, tool, args.clone());
        assert!(text.contains(expect), "{tool}: expected `{expect}` in:\n{text}");
    }
    // Git tools need a repository.
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(&e.root).output().map(|o| o.status.success()).unwrap_or(false)
    };
    if git(&["init", "-q"]) && git(&["add", "-A"]) && git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]) {
        let f = e.root.join("frontend-react/src/utils/hash.ts");
        let src = std::fs::read_to_string(&f).unwrap();
        std::fs::write(&f, src.replace("rounds: number = HASH_ROUNDS", "rounds: number = HASH_ROUNDS, salt = \"\"")).unwrap();
        let diff = ok(&e, "get_structural_diff", json!({}));
        assert!(diff.contains("~ sig fn hash"), "{diff}");
        let sub = ok(&e, "get_changed_subgraph", json!({}));
        assert!(sub.contains("CHANGED fn hash") && sub.contains("AuthStore.login"), "{sub}");
    } else {
        eprintln!("git unavailable: skipping get_structural_diff / get_changed_subgraph");
    }
    assert_eq!(codegraph_mcp::registry().len(), 28);
}

#[test]
fn search_latency_is_low() {
    let (_t, e) = engine();
    let conn = e.reader().unwrap();
    let t0 = Instant::now();
    let n = 200;
    for i in 0..n {
        let q = ["Order", "ItemServ", "cancel", "hash", "Gateway"][i % 5];
        codegraph_graph::query::search(&conn, q, None, 20, false).unwrap();
    }
    let per = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    eprintln!("FTS search: {per:.3} ms/query");
    assert!(per < 25.0, "search too slow: {per} ms");
}
