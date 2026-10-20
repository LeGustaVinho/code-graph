//! Rust language edge cases (fixtures/edge-cases/rust).

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/rust")
}

#[test]
fn items_impls_and_traits() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "OrderRepo").kind, "interface");
    assert!(syms(c, "Status").iter().any(|s| s.kind == "enum"), "{:?}", syms(c, "Status"));
    assert_eq!(sym(c, "Status.Paid").kind, "const", "enum variant");
    assert_eq!(sym(c, "Order.items").kind, "field");
    assert_eq!(sym(c, "Item.0").kind, "field", "tuple struct field");
    let total = sym(c, "Order.total");
    assert_eq!(total.kind, "method");
    assert!(total.signature.starts_with("pub fn total(&self) -> f64"), "{total:?}");
    // `impl OrderRepo for MemoryRepo` makes the struct (not the impl block) implement the trait.
    let implementers = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id          WHERE e.relation = 'INHERITS' AND t.name = 'OrderRepo' AND s.name = 'MemoryRepo' AND s.kind = 'struct'",
    );
    assert_eq!(implementers, 1);
    assert_eq!(syms(c, "Token.0")[0].signature, "u32");
    let token_vis: String = c.query_row("SELECT visibility FROM node WHERE qualified_name = 'Token.0'", [], |r| r.get(0)).unwrap();
    assert_eq!(token_vis, "private", "tuple fields without pub are private");
    assert_eq!(sym(c, "MAX_ITEMS").kind, "const");
}

#[test]
fn use_paths_mod_files_and_crates() {
    let f = fixture();
    let c = &f.conn;
    // `use crate::services::describe` / `mod tests { use super::*; }`.
    assert_eq!(edge(c, "create_order", "describe", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "tests.describes_orders", "describe", "CALLS").as_deref(), Some("exact"), "calls inside assert_eq!");
    assert_eq!(edge(c, "tests.describes_orders", "tests.fixture", "CALLS").as_deref(), Some("exact"));
    // `crate::count_request()` and `models::Order::new` (module import + associated fn).
    assert_eq!(edge(c, "create_order", "count_request", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "OrderService.open", "Order.new", "CALLS").as_deref(), Some("exact"));
    // Another crate of the workspace by its Cargo name (`shared-kit` → `shared_kit`).
    assert_eq!(edge(c, "OrderService.checkout", "round_cents", "CALLS").as_deref(), Some("exact"));
    // Binary crate importing its library by package name.
    assert_eq!(edge(c, "main", "router", "CALLS").as_deref(), Some("exact"));
    let imports = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node t ON t.id = e.target_id WHERE e.relation = 'IMPORTS' \
         AND e.source_file = 'src/lib.rs' AND t.kind = 'module' AND t.file_path = 'src/repo/mod.rs'",
    );
    assert_eq!(imports, 1, "`mod repo;` links lib.rs to repo/mod.rs");
}

#[test]
fn method_calls_through_types_and_traits() {
    let f = fixture();
    let c = &f.conn;
    // `Self::default()` / `repo.save(o)` inside an `impl` in another file than the struct.
    assert!(edge(c, "MemoryRepo.with_seed", "MemoryRepo.save", "CALLS").is_some(), "{:?}", targets(c, "MemoryRepo.with_seed", "CALLS"));
    // `let order = self.repo.find(id)?;` — `Box<dyn OrderRepo>`, then `Result<Order, _>` unwrapped by `?`.
    assert!(edge(c, "OrderService.checkout", "OrderRepo.find", "CALLS").is_some());
    assert!(edge(c, "OrderService.checkout", "MemoryRepo.find", "CALLS").is_some(), "dispatch to the single implementation");
    assert_eq!(
        edge(c, "OrderService.checkout", "Order.total", "CALLS").as_deref(),
        Some("exact"),
        "{:?}",
        targets(c, "OrderService.checkout", "CALLS")
    );
    // `Arc<Mutex<MemoryRepo>>` → `.lock().unwrap().save(…)`.
    assert_eq!(edge(c, "OrderService.checkout", "MemoryRepo.save", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "OrderService.open", "OrderService.log_open", "CALLS").as_deref(), Some("exact"), "Self::log_open");
    // Generic bound `R: OrderRepo` → trait default method.
    assert!(edge(c, "generic_total", "OrderRepo.count", "CALLS").is_some());
    assert!(edge(c, "OrderService.checkout", "OrderService.cache", "READS_VAR").is_some());
}

#[test]
fn errors_routes_config_and_tests() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "OrderService.checkout", "AppError", "THROWS").is_some(), "return Err(AppError::Invalid(..))");
    let route = count(c, "SELECT COUNT(*) FROM api_route WHERE http_method = 'POST' AND path = '/orders'");
    assert_eq!(route, 1, "axum .route(\"/orders\", get(..).post(create_order))");
    let handler = count(
        c,
        "SELECT COUNT(*) FROM api_route r JOIN node n ON n.id = r.handler_id WHERE r.path = '/orders/:id' AND n.name = 'show_order'",
    );
    assert_eq!(handler, 1);
    assert!(sym(c, "main").is_entrypoint, "#[tokio::main]");
    assert!(sym(c, "tests.describes_orders").is_test);
    assert!(sym(c, "tests.fixture").is_test, "helpers in #[cfg(test)] modules are test code");
    assert!(sym(c, "checkout_empty_order_fails").is_test);
    assert!(edge(c, "checkout_empty_order_fails", "OrderService.checkout", "CALLS").is_some(), "integration test → library");
    let cfg = count(c, "SELECT COUNT(*) FROM config_dependency WHERE config_key = 'SHOP_API_URL'");
    assert_eq!(cfg, 1);
    assert!(edge(c, "count_request", "REQUESTS", "WRITES_VAR").is_some(), "static mut write");
    assert!(edge(c, "audit", "count_request", "CALLS").is_some(), "calls in macro_rules! bodies");
    assert!(edge(c, "report", "api_base", "CALLS").is_some(), "calls inside json! objects");
}

#[test]
fn bindings_inline_modules_and_test_crates() {
    let f = fixture();
    let c = &f.conn;
    // `if let Err(e) = repo.find(1)`: e is the error type, not the Ok type.
    assert_eq!(edge(c, "error_code", "AppError.code", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "error_code", "CALLS"));
    assert!(edge(c, "error_code", "Order.code", "CALLS").is_none());
    // Inline `mod util { pub mod fmt_helpers; }` → src/util/fmt_helpers.rs.
    assert_eq!(edge(c, "util.shout", "bang", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "util.shout", "CALLS"));
    assert_eq!(edge(c, "announce", "util.shout", "CALLS").as_deref(), Some("exact"));
    // Two integration tests sharing `tests/common/mod.rs`.
    assert_eq!(edge(c, "opens_orders", "seeded_service", "CALLS").as_deref(), Some("exact"));
    assert!(edge(c, "seeded_service", "OrderService.new", "CALLS").is_some());
}

#[test]
fn diesel_models() {
    let f = fixture();
    let c = &f.conn;
    let tables = count(c, "SELECT COUNT(*) FROM data_model WHERE framework = 'diesel' AND table_name IN ('customers', 'invoices')");
    assert_eq!(tables, 2);
    let fk = count(
        c,
        "SELECT COUNT(*) FROM data_model_field WHERE field_name = 'customer_id' AND is_foreign_key = 1 AND target_model = 'Customer'",
    );
    assert_eq!(fk, 1, "customer_id → Customer by convention");
    assert_eq!(
        count(c, "SELECT COUNT(*) FROM node WHERE name = 'store' AND capabilities LIKE '%DB_WRITE%'"),
        1,
        "diesel::insert_into(invoices::table)"
    );
}
