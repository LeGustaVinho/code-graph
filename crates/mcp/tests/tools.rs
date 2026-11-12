//! MCP tool tests: token budgets, freshness, protocol conformance and a smoke run of all tools.

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
fn flow_evidence_preserves_guards_and_calls_across_languages() {
    let (_tmp, e) = engine();
    for (symbol, must_include) in [
        ("OrderService.PlaceOrderAsync", vec!["if (order.Items.Count", "foreach (var item", "SaveChangesAsync", "item.Quantity = 1", "return order"]),
        ("ItemService.get", vec!["if item_id in CACHE", "self._load(item_id)", "CACHE[item_id] = item", "raise ItemNotFound", "return item"]),
        ("AuthStore.login", vec!["this.user = user", "hash(user", "return currentToken"]),
    ] {
        let condensed = ok(&e, "get_flow_evidence", json!({"symbol_id": symbol}));
        for phrase in must_include {
            assert!(condensed.contains(phrase), "{symbol} missed {phrase}: {condensed}");
        }
        assert!(condensed.contains("selected source excerpt, not exhaustive"), "{condensed}");
    }
    let grouped = ok(&e, "get_flow_evidence", json!({"symbol_id": "OrderService", "filter": "CancelOrderAsync"}));
    assert!(grouped.contains("CancelOrderAsync") && grouped.contains("if (order == null)"), "{grouped}");
    assert!(!grouped.contains("PlaceOrderAsync"), "{grouped}");
}

#[test]
fn flow_evidence_preserves_rust_guards_and_calls() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("rust");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/edge-cases/rust"), &root);
    let e = Engine::open(&root).unwrap();
    let body = ok(&e, "get_flow_evidence", json!({"symbol_id": "OrderService.checkout"}));
    assert!(body.contains("if order.items.is_empty()"), "{body}");
    assert!(body.contains("self.repo.find(id)"), "{body}");
    assert!(body.contains("round_cents(total)"), "{body}");
}

#[test]
fn overload_families_are_navigable_without_multiple_id_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("csharp");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/edge-cases/csharp"), &root);
    let e = Engine::open(&root).unwrap();
    let flow = ok(&e, "get_flow_evidence", json!({"symbol_id": "Params.Write", "max_lines": 12}));
    assert!(flow.contains("2 overloads") && flow.contains("Write(string format)"), "{flow}");
    assert!(flow.contains("Write(string format, params object[] args)"), "{flow}");
    let callers = ok(&e, "get_call_hierarchy", json!({"symbol_id": "Params.Write", "direction": "callers", "depth": 1}));
    assert!(callers.contains("2 overloads") && callers.contains("Write"), "{callers}");
}

#[test]
fn file_outline_filter_keeps_ancestors_without_unrelated_members() {
    let (_tmp, e) = engine();
    let outline = ok(&e, "get_file_outline", json!({
        "file_path": "backend-dotnet/Services/OrderService.cs", "filter": "CancelOrderAsync"
    }));
    assert!(outline.contains("class OrderService") && outline.contains("CancelOrderAsync"), "{outline}");
    assert!(!outline.contains("PlaceOrderAsync") && !outline.contains("ListOrdersAsync"), "{outline}");
    let none = ok(&e, "get_file_outline", json!({
        "file_path": "backend-dotnet/Services/OrderService.cs", "filter": "does_not_exist_13529"
    }));
    assert!(none.contains("no indexed declarations matching"), "{none}");
}

#[test]
fn source_search_covers_supported_languages_scopes_and_absence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    for (relative, source) in [
        ("csharp/Service.cs", "class TokenTarget {}\n// TokenTarget\n"),
        ("typescript/api.ts", "export const TokenTarget = 1;\n"),
        ("javascript/api.js", "const TokenTarget = 3;\n"),
        ("python/service.py", "TokenTarget = 2\n"),
        ("rust/lib.rs", "pub struct TokenTarget;\n"),
    ] {
        let file = root.join(relative);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
    }
    let e = Engine::open(&root).unwrap();
    let all = ok(&e, "search_source", json!({ "query": "TokenTarget", "whole_word": true }));
    for language in ["csharp", "typescript", "javascript", "python", "rust"] {
        assert!(all.contains(language), "missing {language} match: {all}");
    }
    assert!(all.contains("6 matching line(s)"), "{all}");

    let scoped = ok(&e, "search_source", json!({ "query": "TokenTarget", "path": "python" }));
    assert!(scoped.contains("1 matching line(s)"), "{scoped}");
    assert!(scoped.contains("python/service.py:1"), "{scoped}");
    assert!(!scoped.contains("csharp/"), "{scoped}");
    let absent = ok(&e, "search_source", json!({ "query": "TokenTargetExtra", "path": "csharp" }));
    assert!(absent.contains("0 matching line(s)"), "{absent}");

    let partial = ok(&e, "search_source", json!({ "query": "tokentarget", "case_sensitive": false, "limit": 1 }));
    assert!(partial.contains("6 matching line(s)"), "{partial}");
    assert!(partial.contains("[MORE: showing 1 of 6 results"), "{partial}");

    let (missing, failed) = call_tool(&e, "search_source", json!({ "query": "TokenTarget", "path": "unknown" }));
    assert!(failed && missing.contains("does not exist relative to the project root"), "{missing}");
}

#[test]
fn source_search_distinguishes_unindexed_directories_from_verified_indexed_absence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(root.join("Tests")).unwrap();
    std::fs::create_dir_all(root.join("Assets/Tests")).unwrap();
    std::fs::write(root.join(".codegraphignore"), "/Tests/\n").unwrap();
    std::fs::write(root.join("Tests/Ignored.cs"), "class HiddenToken {}\n").unwrap();
    std::fs::write(root.join("Assets/Tests/Included.cs"), "class VisibleToken {}\n").unwrap();
    std::fs::write(root.join("README.md"), "HiddenToken\n").unwrap();
    let e = Engine::open(&root).unwrap();

    // An actual directory with ignored files has not been searched: a zero-match
    // reply would falsely imply that HiddenToken is absent from the directory.
    let (ignored, failed) = call_tool(&e, "search_source", json!({ "query": "HiddenToken", "path": "Tests" }));
    assert!(failed && ignored.contains("exists but has no indexed source files"), "{ignored}");
    assert!(ignored.contains("absence cannot be established"), "{ignored}");
    assert!(ignored.contains("Assets/Tests"), "indexed nested path suggestion missing: {ignored}");

    let (unsupported, failed) = call_tool(&e, "search_source", json!({ "query": "HiddenToken", "path": "README.md" }));
    assert!(failed && unsupported.contains("exists but has no indexed source files"), "{unsupported}");
    let indexed = ok(&e, "search_source", json!({ "query": "VisibleToken", "path": "Assets/Tests" }));
    assert!(indexed.contains("Assets/Tests/Included.cs:1"), "{indexed}");
    let normalized = ok(&e, "search_source", json!({ "query": "VisibleToken", "path": ".\\Assets//Tests\\" }));
    assert!(normalized.contains("Assets/Tests/Included.cs:1"), "{normalized}");
    if cfg!(windows) {
        let folded = ok(&e, "search_source", json!({ "query": "VisibleToken", "path": "assets/tests" }));
        assert!(folded.contains("Assets/Tests/Included.cs:1"), "{folded}");
    }

    let absent = ok(&e, "search_source", json!({ "query": "HiddenToken", "path": "Assets/Tests" }));
    assert!(absent.contains("0 matching line(s) in indexed source"), "{absent}");
    assert!(absent.contains("absence outside the index is unverified"), "{absent}");
    let root_absent = ok(&e, "search_source", json!({ "query": "HiddenToken" }));
    assert!(root_absent.contains("0 matching line(s) in indexed source"), "{root_absent}");
    assert!(root_absent.contains("ignored, unsupported and oversized files were not checked"), "{root_absent}");
}

#[test]
fn source_search_empty_test_tree_is_valid_zero_result() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("Tests/Firestore")).unwrap();
    std::fs::write(root.join(".codegraphignore"), "/Tests/\n").unwrap();
    std::fs::write(root.join("Tests/Firestore/notes.md"), "HiddenToken").unwrap();
    std::fs::write(root.join("Assets.cs"), "class VisibleToken {}").unwrap();
    let e = Engine::open(root).unwrap();

    let result = ok(&e, "search_source", json!({"query":"HiddenToken","path":"Tests"}));
    assert!(result.contains("0 matching line(s)"), "{result}");
    assert!(result.contains("verified empty source scope"), "{result}");
    assert!(result.contains("0 indexed files scanned"), "{result}");

    // A file that the index would support must not silently become a
    // verified zero just because the project excluded it from indexing.
    std::fs::write(root.join("Tests/Firestore/Hidden.cs"), "class HiddenToken {}").unwrap();
    let (message, failed) = call_tool(&e, "search_source", json!({"query":"HiddenToken","path":"Tests"}));
    assert!(failed && message.contains("no indexed source files"), "{message}");
    assert!(message.contains("absence cannot be established"), "{message}");
}

#[test]
fn universal_search_exact_restricts_matches_to_case_sensitive_declaration_names() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Panels.cs"), "public interface IPanelDataHandler { void BindPanelData(); }\npublic class PanelDataHandler : IPanelDataHandler { public void BindPanelData() {} public void BindPanelDataExtra() {} }\n").unwrap();
    let e = Engine::open(&root).unwrap();

    let fuzzy = ok(&e, "search_universal", json!({ "query": "PanelDataHandler" }));
    assert!(fuzzy.contains("IPanelDataHandler"), "fuzzy default should still include substring hits: {fuzzy}");

    let exact = ok(&e, "search_universal", json!({ "query": "PanelDataHandler", "exact": true }));
    assert!(exact.contains("exact declaration match(es)"), "{exact}");
    assert!(exact.contains("class PanelDataHandler"), "{exact}");
    assert!(!exact.contains("interface IPanelDataHandler"), "{exact}");

    let member = ok(&e, "search_universal", json!({ "query": "PanelDataHandler.BindPanelData", "exact": true, "kind": "method" }));
    assert!(member.contains("1 exact declaration match(es)"), "{member}");
    assert!(!member.contains("IPanelDataHandler.BindPanelData"), "{member}");
    assert!(!member.contains("BindPanelDataExtra"), "{member}");
    let wrong_kind = ok(&e, "search_universal", json!({ "query": "PanelDataHandler.BindPanelData", "exact": true, "kind": "class" }));
    assert!(wrong_kind.contains("0 exact declaration match(es)"), "{wrong_kind}");

    let case = ok(&e, "search_universal", json!({ "query": "paneldatahandler.bindpaneldata", "exact": true }));
    assert!(case.contains("0 exact declaration match(es)"), "{case}");
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
    assert_eq!(tools.len(), 32);
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
fn all_tools_answer() {
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
        ("get_project_overview", json!({}), "Code areas:"),
        ("find_files", json!({ "query": "OrderService.cs" }), "backend-dotnet/Services/OrderService.cs"),
        ("search_universal", json!({ "query": "ItemServ" }), "class ItemService"),
        ("find_literal", json!({ "query": "/api/orders", "exact": true }), "frontend-react/src/api/orders.ts:11"),
        ("find_variable_or_const", json!({ "name": "CACHE" }), "W backend-python/app/services.py:20"),
        ("trace_execution_path", json!({ "source": "OrderList", "target": "OrderService.CancelOrderAsync" }), "4 hop(s)"),
        ("trace_fullstack_flow", json!({ "api_route_or_call": "GET /api/orders" }), "ROUTE GET /api/Orders"),
        ("resolve_dependency_injection", json!({ "interface_or_token": "IClock" }), "IClock → SystemClock"),
        ("get_call_hierarchy", json!({ "symbol_id": "OrderService.CancelOrderAsync", "direction": "callers" }), "OrdersController.Cancel"),
        ("get_type_hierarchy", json!({ "symbol_id": "OrderService" }), "implements iface IOrderService"),
        ("get_impact_radius", json!({ "symbol_id": "hash" }), "AuthStore.login"),
        ("get_state_mutations", json!({ "symbol_id": "OrderService.PlaceOrderAsync" }), "database Orders"),
        ("trace_error_flow", json!({ "exception_name_or_message": "ItemNotFound" }), "CAUGHT by global handler fn handle_not_found"),
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
    assert_eq!(codegraph_mcp::registry().len(), 32);
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

#[test]
fn compact_outputs_group_sites_and_tests() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("tests")).unwrap();
    std::fs::write(
        root.join("src/limits.ts"),
        "export function rate(key: string) { return key; }\n\
         export function command() {\n  rate('a');\n  rate('b');\n}\n\
         export function setup() { command(); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tests/limits.test.ts"),
        "import { rate } from '../src/limits';\nimport { setup } from '../src/limits';\n\
         test('rate keys', () => { rate('x'); });\ntest('setup wires commands', () => { setup(); });\n",
    )
    .unwrap();
    let e = Engine::open(&root).unwrap();
    // Every call site of one caller on one line, without restating exact edges' evidence.
    let callers = ok(&e, "get_call_hierarchy", json!({ "symbol_id": "rate", "direction": "callers", "depth": 1 }));
    assert!(callers.contains("command src/limits.ts:2-5 @3,4"), "{callers}");
    assert!(!callers.contains("[exact"), "{callers}");
    // Tests that call the symbol are listed first.
    let tests = ok(&e, "get_tests_for_symbol", json!({ "symbol_id": "rate" }));
    assert!(tests.contains("closest (1)") && tests.contains("direct \"rate keys\""), "{tests}");
    let limited = ok(&e, "get_tests_for_symbol", json!({ "symbol_id": "rate", "limit": 1, "max_token_budget": 100 }));
    assert!(estimate_tokens(&limited) <= 100, "{limited}");
    let rich_sites = ok(&e, "get_call_hierarchy", json!({ "symbol_id": "rate", "direction": "callers", "depth": 1, "show_sites": true }));
    assert!(rich_sites.contains("rate('a')") && rich_sites.contains("rate('b')"), "{rich_sites}");
    // A result list shorter than the limit is complete: no truncation marker.
    let search = ok(&e, "search_universal", json!({ "query": "rate", "limit": 10 }));
    assert!(!search.contains("TRUNCATED") && !search.contains("MORE"), "{search}");
}

#[test]
fn skeleton_respects_esm_exports_and_other_languages() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/api.ts"),
        "class Hidden { unused() { return 0; } }\n\
         function helper() { return 0; }\n\
         export class Visible { call() { return 1; } private secret() { return 2; } }\n\
         export function publicEntry() { return 3; }\n").unwrap();
    std::fs::write(root.join("src/script.js"),
        "function runtimeEntry() { return 1; }\n\
         function helper() { return runtimeEntry(); }\n").unwrap();
    std::fs::write(root.join("src/service.py"),
        "class Worker:\n    def run(self):\n        return True\n").unwrap();
    let e = Engine::open(&root).unwrap();
    let esm = ok(&e, "get_file_skeleton", json!({ "file_path": "src/api.ts" }));
    assert!(esm.contains("Visible") && esm.contains("call(") && esm.contains("publicEntry"), "{esm}");
    assert!(!esm.contains("Hidden") && !esm.contains("helper") && !esm.contains("secret("), "{esm}");
    let script = ok(&e, "get_file_skeleton", json!({ "file_path": "src/script.js" }));
    assert!(script.contains("runtimeEntry"), "{script}");
    let python = ok(&e, "get_file_skeleton", json!({ "file_path": "src/service.py" }));
    assert!(python.contains("class Worker") && python.contains("run("), "{python}");
}

#[test]
fn skeleton_reports_hidden_private_members_and_can_include_them() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Actor.cs"),
        "public class Actor {\n\
             public void Activate() {}\n\
             void Start() {}\n\
             private void OnDestroy() {}\n\
             protected void OnEnable() {}\n\
             private class State { public void Reset() {} }\n\
         }\n").unwrap();
    std::fs::write(root.join("module.py"), "def _hidden():\n    return 1\n").unwrap();
    std::fs::write(root.join("api.ts"),
        "class Unexported { show() {} }\n\
         export class Exported { public show() {} private secret() {} }\n").unwrap();
    let e = Engine::open(&root).unwrap();

    let public = ok(&e, "get_file_skeleton", json!({ "file_path": "Actor.cs" }));
    assert!(public.contains("class Actor") && public.contains("Activate()") && public.contains("OnEnable()"), "{public}");
    assert!(!public.contains("Start()") && !public.contains("OnDestroy()") && !public.contains("class State"), "{public}");
    assert!(public.contains("private declaration(s) omitted") && public.contains("include_private=true"), "{public}");
    let complete = ok(&e, "get_file_skeleton", json!({ "file_path": "Actor.cs", "include_private": true }));
    for expected in ["Start(): void", "OnDestroy(): void", "class State", "Reset(): void"] {
        assert!(complete.contains(expected), "missing {expected}: {complete}");
    }
    assert!(complete.contains("[private]") && !complete.contains("private declaration(s) omitted"), "{complete}");

    let hidden_only = ok(&e, "get_file_skeleton", json!({ "file_path": "module.py" }));
    assert!(hidden_only.contains("private declaration(s) omitted") && hidden_only.contains("(no declarations in selected scope)"), "{hidden_only}");
    assert!(!hidden_only.contains("def _hidden"), "{hidden_only}");
    let python_full = ok(&e, "get_file_skeleton", json!({ "file_path": "module.py", "include_private": true }));
    assert!(python_full.contains("_hidden(") && python_full.contains("[private]"), "{python_full}");

    let esm = ok(&e, "get_file_skeleton", json!({ "file_path": "api.ts", "include_private": true }));
    assert!(esm.contains("Exported") && esm.contains("secret()") && esm.contains("[private]"), "{esm}");
    assert!(!esm.contains("Unexported"), "{esm}");
    assert!(esm.contains("non-exported ES module declaration(s) omitted"), "{esm}");

    let limited = ok(&e, "get_file_skeleton", json!({ "file_path": "Actor.cs", "max_token_budget": 125 }));
    assert!(estimate_tokens(&limited) <= 125, "{limited}");
    assert!(limited.contains("private declaration(s) omitted"), "{limited}");
}

#[test]
fn config_matrix_scopes_paths_and_key_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("apps/api")).unwrap();
    std::fs::create_dir_all(root.join("apps/web")).unwrap();
    std::fs::write(root.join("apps/api/config.ts"), "export function config() { return process.env.API_PORT ?? 3000; }\n").unwrap();
    std::fs::write(root.join("apps/web/config.ts"), "export function config() { return process.env.WEB_PORT ?? 5173; }\n").unwrap();
    let e = Engine::open(&root).unwrap();
    let scoped = ok(&e, "get_config_matrix", json!({ "path": "apps\\api" }));
    assert!(scoped.contains("API_PORT") && scoped.contains("apps/api"), "{scoped}");
    assert!(!scoped.contains("WEB_PORT") && !scoped.contains("apps/web"), "{scoped}");
    let filtered = ok(&e, "get_config_matrix", json!({ "filter": "WEB_PORT" }));
    assert!(filtered.contains("WEB_PORT") && !filtered.contains("API_PORT"), "{filtered}");
    let json_result = ok(&e, "get_config_matrix", json!({ "path": "apps/api", "format": "json" }));
    assert!(json_result.contains("\"key\":\"API_PORT\"") && !json_result.contains("WEB_PORT"), "{json_result}");
    let budgeted = ok(&e, "get_config_matrix", json!({ "max_token_budget": 100 }));
    assert!(estimate_tokens(&budgeted) <= 100, "{budgeted}");
}

#[test]
fn symbol_body_absolute_ranges_work_across_languages() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("proj");
    std::fs::create_dir_all(root.join("src")).unwrap();
    // Each declaration spans over 200 lines. The same absolute file coordinates must
    // work for top-level functions and methods, independent of language/indentation.
    let cases = [
        ("long.ts", "tsLong", "export function tsLong() {\n", "    const ts_step_", "}\n"),
        ("long.js", "jsLong", "function jsLong() {\n", "    let js_step_", "}\n"),
        ("long.py", "PyWorker.expand", "class PyWorker:\n    def expand(self):\n", "        py_step_", ""),
        ("long.rs", "rust_long", "pub fn rust_long() {\n", "    let rs_step_", "}\n"),
        ("long.cs", "CsWorker.Expand", "public class CsWorker {\n    public void Expand() {\n", "        int cs_step_", "    }\n}\n"),
    ];
    let mut files = Vec::new();
    for (file, symbol, opening, prefix, closing) in cases {
        let mut src = "\n".repeat(5);
        src.push_str(opening);
        for i in 0..200 {
            src.push_str(&format!("{prefix}{i} = {i};\n"));
        }
        src.push_str(closing);
        std::fs::write(root.join("src").join(file), &src).unwrap();
        files.push((file, symbol, src));
    }
    let e = Engine::open(&root).unwrap();
    for (file, symbol, src) in &files {
        let output = ok(&e, "get_symbol_body", json!({
            "symbol_id": symbol, "start_line": 35, "end_line": 50
        }));
        assert!(output.contains("file lines 35..50 (absolute, inclusive)"), "{file}: {output}");
        for line in 35..=50 {
            let original = src.lines().nth(line - 1).unwrap();
            assert!(output.contains(&format!("{line:>4}| {original}")),
                "{file}: missing exact file line {line} in\n{output}");
        }
        assert!(!output.contains("  34| ") && !output.contains("  51| "), "{file}: {output}");
        assert!(estimate_tokens(&output) < 500, "{file}: unexpectedly verbose:\n{output}");

        // Previous callers with only max_lines still get the original prefix.
        let default = ok(&e, "get_symbol_body", json!({ "symbol_id": symbol, "max_lines": 2 }));
        assert!(!default.contains("file lines 35..50"), "{file}: {default}");
        assert!(default.contains("[TRUNCATED: showing 2 of"), "{file}: {default}");
        let capped = ok(&e, "get_symbol_body", json!({
            "symbol_id": symbol, "start_line": 35, "end_line": 50, "max_lines": 3
        }));
        assert!(capped.contains("[TRUNCATED: showing 3 of 16 lines]"), "{file}: {capped}");
        assert!(!capped.contains("  38| "), "{file}: {capped}");
    }
}

#[test]
fn symbol_body_absolute_ranges_reject_invalid_coordinates() {
    let (_tmp, e) = engine();
    // Fixture's C# method spans lines 52..62. Selecting 50..54 would expose
    // unrelated source from the same file, so reject instead of clamping.
    for (args, explanation) in [
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "start_line": 50, "end_line": 54}), "inside symbol"),
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "start_line": 55, "end_line": 63}), "inside symbol"),
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "start_line": 57, "end_line": 56}), "ordered"),
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "start_line": 0, "end_line": 55}), "positive"),
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "start_line": 55}), "provided together"),
        (json!({"symbol_id": "OrderService.CancelOrderAsync", "end_line": 55}), "provided together"),
    ] {
        let (message, is_error) = call_tool(&e, "get_symbol_body", args.clone());
        assert!(is_error && message.contains(explanation), "{args}: {message}");
    }
    // A range equal to the symbol's endpoints remains valid.
    let full = ok(&e, "get_symbol_body", json!({
        "symbol_id": "OrderService.CancelOrderAsync", "start_line": 52, "end_line": 62
    }));
    assert!(full.contains("file lines 52..62") && full.contains("  52| "), "{full}");
}
