//! Determinism: an index built incrementally through a sequence of edits must be
//! identical to one rebuilt from scratch, both in the stored graph and in every MCP
//! tool answer.

use std::path::{Path, PathBuf};

use codegraph_graph::{index_project, open, IndexOptions};
use rusqlite::types::ValueRef;
use rusqlite::Connection;
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

/// Bumps mtime-sensitive state reliably (some filesystems have coarse mtimes).
fn write(path: &Path, content: &str) {
    std::thread::sleep(std::time::Duration::from_millis(20));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn edit(root: &Path, rel: &str, from: &str, to: &str) {
    let p = root.join(rel);
    // Checkouts may use CRLF; edit on LF so the patterns below stay portable.
    let src = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
    assert!(src.contains(from), "{rel} does not contain {from:?}");
    write(&p, &src.replacen(from, to, 1));
}

fn render(v: ValueRef) -> String {
    match v {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => format!("{f:?}"),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("blob:{}", b.len()),
    }
}

fn rows(conn: &Connection, sql: &str) -> Vec<String> {
    let mut st = conn.prepare(sql).unwrap();
    let n = st.column_count();
    let mut out: Vec<String> = st
        .query_map([], |r| Ok((0..n).map(|i| render(r.get_ref(i).unwrap())).collect::<Vec<_>>().join(" | ")))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    out.sort();
    out
}

/// Columns that legitimately differ between equivalent databases.
fn volatile(table: &str, col: &str, rowid_pk: bool) -> bool {
    rowid_pk || (table == "file_meta" && matches!(col, "last_mtime" | "last_indexed_at"))
}

/// Canonical, row-id and timestamp free dump of the graph, keyed by table.
fn dump(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    // Edges, readable: endpoints by qualified name (plus ids, which are content-addressed).
    out.push((
        "edge".to_string(),
        rows(
            conn,
            "SELECT COALESCE(s.qualified_name, '#' || e.source_id), e.source_id, \
                    COALESCE(t.qualified_name, '?' || e.unresolved_symbol, '#' || e.target_id), e.target_id, \
                    e.relation, e.confidence, e.evidence, e.resolver, e.line_number, e.metadata, e.source_file, e.target_file, e.derived \
             FROM edge e LEFT JOIN node s ON s.id = e.source_id LEFT JOIN node t ON t.id = e.target_id",
        ),
    ));
    let tables: Vec<String> = rows(
        conn,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT IN ('edge', 'meta') AND name NOT LIKE '%fts%' AND name NOT LIKE 'sqlite_%'",
    )
    .into_iter()
    .map(|t| t.trim_matches('"').to_string())
    .collect();
    for t in tables {
        let mut st = conn.prepare(&format!("PRAGMA table_info(\"{t}\")")).unwrap();
        let cols: Vec<String> = st
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(5)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .filter(|(name, ty, pk)| !volatile(&t, name, *pk == 1 && ty.eq_ignore_ascii_case("INTEGER")))
            .map(|(name, _, _)| format!("\"{name}\""))
            .collect();
        out.push((t.clone(), rows(conn, &format!("SELECT {} FROM \"{t}\"", cols.join(", ")))));
    }
    out
}

fn assert_same_dump(step: &str, inc: &Connection, full: &Connection) {
    let (a, b) = (dump(inc), dump(full));
    assert_eq!(a.iter().map(|t| &t.0).collect::<Vec<_>>(), b.iter().map(|t| &t.0).collect::<Vec<_>>());
    let mut report = String::new();
    for ((table, ra), (_, rb)) in a.iter().zip(b.iter()) {
        if ra == rb {
            continue;
        }
        let only_a: Vec<&String> = ra.iter().filter(|r| rb.binary_search(r).is_err()).take(15).collect();
        let only_b: Vec<&String> = rb.iter().filter(|r| ra.binary_search(r).is_err()).take(15).collect();
        report.push_str(&format!("\n== {table}: incremental {} rows, rebuild {} rows\n", ra.len(), rb.len()));
        for r in only_a {
            report.push_str(&format!("  - incremental only: {r}\n"));
        }
        for r in only_b {
            report.push_str(&format!("  + rebuild only:     {r}\n"));
        }
    }
    assert!(report.is_empty(), "after step `{step}` the incremental index differs from a rebuild:{report}");
}

/// Removes timings like `(1.2 ms)` and the absolute project root.
fn normalize(text: &str, root: &Path) -> String {
    let root = root.display().to_string();
    let s = text.replace(&root, "<ROOT>").replace(&root.replace('\\', "/"), "<ROOT>");
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let num = after.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(after.len());
        if num > 0 && after[num..].starts_with(" ms)") {
            out.push_str("(<t> ms)");
            rest = &after[num + 4..];
        } else {
            out.push('(');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn tool_calls() -> Vec<(&'static str, Value)> {
    vec![
        (
            "batch_query",
            json!({ "operations": [{ "tool": "get_file_outline", "args": { "file_path": "auth.ts" } }, { "tool": "get_entrypoints" }] }),
        ),
        ("get_file_skeleton", json!({ "file_path": "backend-python/app/services.py" })),
        ("simulate_signature_change", json!({ "symbol_id": "OrderService.PlaceOrderAsync", "new_param_count": 3 })),
        ("get_flow_diagram", json!({ "symbols": ["OrderList", "OrderService.CancelOrderAsync"] })),
        ("search_universal", json!({ "query": "Order" })),
        ("search_universal", json!({ "query": "Handler", "limit": 50 })),
        ("search_universal", json!({ "query": "ItemServ" })),
        ("find_literal", json!({ "query": "/api/orders", "exact": true })),
        ("find_variable_or_const", json!({ "name": "CACHE" })),
        ("trace_execution_path", json!({ "source": "OrderList", "target": "OrderService.CancelOrderAsync" })),
        ("trace_fullstack_flow", json!({ "api_route_or_call": "GET /api/orders" })),
        ("resolve_dependency_injection", json!({ "interface_or_token": "IClock" })),
        ("resolve_dependency_injection", json!({ "interface_or_token": "IHandler" })),
        ("get_call_hierarchy", json!({ "symbol_id": "OrderService.CancelOrderAsync", "direction": "callers" })),
        ("get_call_hierarchy", json!({ "symbol_id": "Dispatcher.Dispatch", "direction": "callees" })),
        ("get_type_hierarchy", json!({ "symbol_id": "OrderService" })),
        ("get_type_hierarchy", json!({ "symbol_id": "IHandler" })),
        ("get_impact_radius", json!({ "symbol_id": "hash" })),
        ("get_impact_radius", json!({ "symbol_id": "digest" })),
        ("get_impact_radius", json!({ "symbol_id": "ItemService" })),
        ("get_state_mutations", json!({ "symbol_id": "OrderService.PlaceOrderAsync" })),
        ("trace_error_flow", json!({ "exception_name_or_message": "ItemNotFound" })),
        ("trace_error_flow", json!({ "exception_name_or_message": "OrderNotFoundException" })),
        ("find_by_capability", json!({ "tags": ["NETWORK_IO"] })),
        ("get_entrypoints", json!({})),
        ("find_orphaned_symbols", json!({})),
        ("get_data_model", json!({})),
        ("get_data_model", json!({ "model_name": "Invoice" })),
        ("get_config_matrix", json!({})),
        ("get_type_contract", json!({ "type_id": "PaymentGateway" })),
        ("get_symbol_body", json!({ "symbol_id": "get_db" })),
        ("get_file_outline", json!({ "file_path": "OrderList.tsx" })),
        ("get_file_outline", json!({ "file_path": "Dispatcher.cs" })),
        ("get_complexity_hotspots", json!({ "min_complexity": 1 })),
        ("validate_dependency_rule", json!({ "source_module": "domain", "target_module": "infrastructure" })),
        ("get_tests_for_symbol", json!({ "symbol_id": "AuthStore.login" })),
        ("get_tests_for_symbol", json!({ "symbol_id": "ItemService" })),
        ("get_structural_diff", json!({})),
        ("get_changed_subgraph", json!({})),
    ]
}

fn assert_same_tools(step: &str, inc_root: &Path, full_root: &Path) {
    let inc = codegraph_mcp::Engine::open(inc_root).unwrap();
    let full = codegraph_mcp::Engine::open(full_root).unwrap();
    let mut called: std::collections::BTreeSet<&str> = Default::default();
    for (tool, args) in tool_calls() {
        called.insert(tool);
        let (a, ea) = codegraph_mcp::call_tool(&inc, tool, args.clone());
        let (b, eb) = codegraph_mcp::call_tool(&full, tool, args.clone());
        let (a, b) = (normalize(&a, &inc.root), normalize(&b, &full.root));
        assert_eq!(ea, eb, "step `{step}`: {tool} {args} error flag differs");
        assert!(a == b, "step `{step}`: {tool} {args} differs\n--- incremental ---\n{a}\n--- rebuild ---\n{b}");
    }
    let all: std::collections::BTreeSet<&str> = codegraph_mcp::registry().iter().map(|t| t.name).collect();
    let missing: Vec<&&str> = all.difference(&called).collect();
    assert!(missing.is_empty(), "tools not covered by the determinism test: {missing:?}");
}

#[test]
fn incremental_index_matches_full_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let inc_root = tmp.path().join("inc");
    copy_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fullstack"), &inc_root);
    let mut inc = open(&inc_root).unwrap();
    index_project(&inc_root, &mut inc, IndexOptions::default()).unwrap();

    type Step = (&'static str, fn(&Path));
    let steps: Vec<Step> = vec![
        ("baseline", |_| {}),
        ("add a 4th IHandler implementation", |r| {
            edit(
                r,
                "backend-dotnet/Handlers/Dispatcher.cs",
                "    public class Dispatcher\n",
                "    public class WebhookHandler : IHandler\n    {\n        public void Execute(string message) { }\n    }\n\n    public class Dispatcher\n",
            )
        }),
        ("rename hash to digest", |r| edit(r, "frontend-react/src/utils/hash.ts", "export function hash(", "export function digest(")),
        ("add a second OrderService in another namespace", |r| {
            write(
                &r.join("backend-dotnet/Legacy/OrderService.cs"),
                "namespace Shop.Legacy\n{\n    public class OrderService\n    {\n        public void CancelOrderAsync(string id) { }\n\n        public int PlaceOrderAsync(int priority)\n        {\n            CancelOrderAsync(\"x\");\n            return priority;\n        }\n    }\n}\n",
            )
        }),
        ("make the app.services suffix ambiguous", |r| {
            write(
                &r.join("backend-python/other/app/services.py"),
                "class ItemService:\n    def get(self, item_id: int):\n        return item_id\n\n\nclass OtherService:\n    def run_job(self):\n        return ItemService().get(1)\n",
            )
        }),
        ("delete a file", |r| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            std::fs::remove_file(r.join("frontend-react/src/components/OrderRow.tsx")).unwrap();
        }),
        ("add a DI registration", |r| {
            edit(
                r,
                "backend-dotnet/Program.cs",
                "builder.Services.AddTransient<Dispatcher>();\n",
                "builder.Services.AddTransient<Dispatcher>();\nbuilder.Services.AddScoped<IHandler, SmsHandler>();\n",
            )
        }),
    ];

    for (i, (step, apply)) in steps.iter().enumerate() {
        apply(&inc_root);
        let stats = index_project(&inc_root, &mut inc, IndexOptions::default()).unwrap();
        assert_eq!(stats.parse_errors, 0, "{step}: {stats:?}");
        eprintln!(
            "{step}: parsed {} removed {} affected {} full_resolution {}",
            stats.files_parsed, stats.files_removed, stats.affected_files, stats.full_resolution
        );
        if i > 0 && i < steps.len() - 1 {
            assert!(!stats.full_resolution, "`{step}` should exercise the incremental path: {stats:?}");
        }

        let full_root = tmp.path().join(format!("full{i}"));
        copy_dir(&inc_root, &full_root);
        let mut full = open(&full_root).unwrap();
        index_project(&full_root, &mut full, IndexOptions { rebuild: true }).unwrap();

        assert_same_dump(step, &inc, &full);
        drop(full);
        assert_same_tools(step, &inc_root, &full_root);
    }
}
