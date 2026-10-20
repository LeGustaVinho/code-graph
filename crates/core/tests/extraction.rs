//! Parsing / extraction tests over the fullstack fixture project.

use std::path::{Path, PathBuf};

use codegraph_core::{extract_file, FileFacts, Language, RefKind, SymbolKind};

fn fixture(rel: &str) -> (String, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/fullstack");
    let src = std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    (rel.to_string(), src)
}

fn facts(rel: &str) -> FileFacts {
    let (path, src) = fixture(rel);
    let lang = Language::from_path(Path::new(&path)).expect("supported language");
    extract_file(&path, lang, &src)
}

fn sym<'a>(f: &'a FileFacts, qname_suffix: &str) -> &'a codegraph_core::Symbol {
    f.symbols
        .iter()
        .find(|s| s.qualified_name == qname_suffix || s.qualified_name.ends_with(&format!(".{qname_suffix}")))
        .unwrap_or_else(|| panic!("symbol {qname_suffix} not found in {}", f.path))
}

#[test]
fn csharp_symbols_lines_and_complexity() {
    let f = facts("backend-dotnet/Services/OrderService.cs");
    let class = sym(&f, "OrderService");
    assert_eq!(class.kind, SymbolKind::Class);
    assert_eq!(class.bases, vec!["IOrderService".to_string()]);
    assert_eq!((class.start_line, class.end_line), (8, 68));
    let place = sym(&f, "OrderService.PlaceOrderAsync");
    assert_eq!(place.kind, SymbolKind::Method);
    assert_eq!((place.start_line, place.end_line), (26, 50));
    assert_eq!(place.complexity, 7, "if(||) + foreach + if(&&) + else-if");
    for cap in ["ASYNC", "DB_WRITE", "THROWS"] {
        assert!(place.capabilities.contains(cap), "missing {cap}: {:?}", place.capabilities);
    }
    let p = place.params.as_ref().unwrap();
    assert_eq!((p.total, p.required), (2, 2));
    assert_eq!(sym(&f, "MaxItems").kind, SymbolKind::Const);
    // Typed receiver from a field declaration.
    let save = f.refs.iter().find(|r| r.kind == RefKind::Call && r.name == "SaveChangesAsync").unwrap();
    assert_eq!(save.receiver_type.as_deref(), Some("AppDbContext"));
    let throw = f.refs.iter().find(|r| r.kind == RefKind::Throw && r.name == "ArgumentException").unwrap();
    assert_eq!(throw.line, 30);
    assert!(throw.detail.as_deref().unwrap().contains("between 1 and 50"));
    assert!(f.mutations.iter().any(|m| m.mutation_type == "database" && m.target == "Orders"));
    assert!(f.mutations.iter().any(|m| m.mutation_type == "local_field" && m.target == "OrderService._placedCount"));
}

#[test]
fn csharp_program_di_routes_config() {
    let f = facts("backend-dotnet/Program.cs");
    let di: Vec<(String, Option<String>, String)> =
        f.di.iter().map(|d| (d.service.clone(), d.implementation.clone(), d.lifetime.clone())).collect();
    assert!(di.contains(&("IOrderService".into(), Some("OrderService".into()), "scoped".into())));
    assert!(di.contains(&("IClock".into(), Some("SystemClock".into()), "singleton".into())));
    assert!(f.routes.iter().any(|r| r.path == "/api/health" && r.http_method == "GET"));
    let keys: Vec<&str> = f.config.iter().map(|c| c.key.as_str()).collect();
    assert!(keys.contains(&"ConnectionStrings:Default") && keys.contains(&"Jwt:Key") && keys.contains(&"SHOP_REGION"));
    assert!(f.symbols[0].is_entrypoint, "top-level statements make Program.cs an entrypoint");
    assert_eq!(f.global_handlers.len(), 1);
}

#[test]
fn csharp_controller_routes_and_try_catch() {
    let f = facts("backend-dotnet/Controllers/OrdersController.cs");
    let paths: Vec<(String, String)> = f.routes.iter().map(|r| (r.http_method.clone(), r.path.clone())).collect();
    assert!(paths.contains(&("POST".into(), "/api/Orders/{id}/cancel".into())));
    assert!(paths.contains(&("GET".into(), "/api/Orders".into())));
    let call = f.refs.iter().find(|r| r.name == "CancelOrderAsync").unwrap();
    assert_eq!(call.catch_types, vec!["OrderNotFoundException".to_string()]);
    assert_eq!(call.receiver_type.as_deref(), Some("IOrderService"));
    assert!(sym(&f, "OrdersController.Cancel").capabilities.contains("AUTH_REQUIRED"), "class-level [Authorize]");
}

#[test]
fn typescript_http_calls_jsx_and_tests() {
    let f = facts("frontend-react/src/api/orders.ts");
    let calls: Vec<(String, String)> = f.http_calls.iter().map(|h| (h.http_method.clone(), h.url.clone())).collect();
    assert!(calls.contains(&("GET".into(), "/api/orders".into())));
    assert!(calls.contains(&("POST".into(), "/api/orders/${id}/cancel".into())));
    assert!(sym(&f, "getOrders").is_exported);

    let list = facts("frontend-react/src/components/OrderList.tsx");
    assert!(list.refs.iter().any(|r| r.kind == RefKind::Jsx && r.name == "OrderRow"));
    assert!(list.imports.iter().any(|i| i.module == "../api/orders" && i.imported.as_deref() == Some("cancelOrder")));

    let t = facts("frontend-react/src/__tests__/auth.test.ts");
    let tests: Vec<&str> = t.symbols.iter().filter(|s| s.kind == SymbolKind::Test).map(|s| s.name.as_str()).collect();
    assert_eq!(tests, vec!["AuthStore", "returns a token on login"]);
    let login = t.refs.iter().find(|r| r.name == "login").unwrap();
    assert_eq!(login.receiver_type.as_deref(), Some("AuthStore"), "type inferred from `new AuthStore()`");
}

#[test]
fn typescript_nestjs_routes_and_di() {
    let c = facts("backend-nest/src/invoices/invoices.controller.ts");
    assert!(c.routes.iter().any(|r| r.http_method == "POST" && r.path == "/invoices/:id/pay"));
    assert!(sym(&c, "InvoicesController.pay").capabilities.contains("AUTH_REQUIRED"));
    let call = c.refs.iter().find(|r| r.name == "pay" && r.kind == RefKind::Call).unwrap();
    assert_eq!(call.receiver_type.as_deref(), Some("InvoicesService"), "constructor parameter property type");
    let m = facts("backend-nest/src/app.module.ts");
    assert!(m.di.iter().any(|d| d.service == "PAYMENT_GATEWAY" && d.implementation.as_deref() == Some("StripeGateway")));
    let main = facts("backend-nest/src/main.ts");
    assert_eq!(main.route_prefixes, vec!["api".to_string()]);
    assert!(main.config.iter().any(|c| c.key == "PORT" && c.default_value.as_deref() == Some("3000")));
}

#[test]
fn python_routes_di_errors_and_fields() {
    let f = facts("backend-python/app/main.py");
    assert!(f.routes.iter().any(|r| r.path == "/api/items/{item_id}" && r.framework == "fastapi"));
    assert!(f.di.iter().any(|d| d.framework == "fastapi" && d.implementation.as_deref() == Some("get_db")));
    assert!(f.config.iter().any(|c| c.key == "DATABASE_URL" && c.default_value.is_some()));
    assert!(f.global_handlers.iter().any(|g| g.exception == "ItemNotFound"));
    assert!(f.symbols[0].is_entrypoint, "__main__ guard");

    let s = facts("backend-python/app/services.py");
    let get = sym(&s, "ItemService.get");
    assert_eq!(get.complexity, 3);
    assert_eq!(get.params.as_ref().unwrap().total, 1, "self is excluded");
    assert!(s.refs.iter().any(|r| r.kind == RefKind::Throw && r.name == "ItemNotFound"));
    assert!(s.symbols.iter().any(|x| x.kind == SymbolKind::Field && x.name == "lookups"));
    assert!(s.mutations.iter().any(|m| m.mutation_type == "global_var" && m.target == "CACHE"));
}

#[test]
fn prisma_and_config_files() {
    let p = facts("backend-nest/prisma/schema.prisma");
    let invoice = sym(&p, "Invoice");
    assert_eq!(invoice.kind, SymbolKind::DataModel);
    assert_eq!((invoice.start_line, invoice.end_line), (12, 18));
    assert!(p.config.iter().any(|c| c.key == "DATABASE_URL"));
    let a = facts("backend-dotnet/appsettings.json");
    let jwt = a.config.iter().find(|c| c.key == "Jwt:Key").unwrap();
    assert_eq!((jwt.usage.as_str(), jwt.line), ("define", 6));
}

#[test]
fn symbol_ids_are_stable_and_hashes_change_with_content() {
    let (path, src) = fixture("frontend-react/src/utils/hash.ts");
    let a = extract_file(&path, Language::TypeScript, &src);
    let b = extract_file(&path, Language::TypeScript, &format!("// header comment\n\n{src}"));
    let id = |f: &FileFacts, n: &str| f.symbols.iter().find(|s| s.name == n && s.kind != SymbolKind::Module).unwrap().id.clone();
    assert_eq!(id(&a, "hash"), id(&b, "hash"), "ids do not depend on line positions");
    let body = |f: &FileFacts, n: &str| f.symbols.iter().find(|s| s.name == n && s.kind != SymbolKind::Module).unwrap().body_hash.clone();
    assert_eq!(body(&a, "hash"), body(&b, "hash"), "comments do not change the body hash");
    assert_ne!(codegraph_core::hash::content_hash(src.as_bytes()), codegraph_core::hash::content_hash(format!("{src} ").as_bytes()));
    let start = |f: &FileFacts, n: &str| f.symbols.iter().find(|s| s.name == n && s.kind != SymbolKind::Module).unwrap().start_line;
    assert_eq!(start(&b, "hash"), start(&a, "hash") + 2);
}

#[test]
fn malformed_sources_do_not_panic() {
    for (path, lang) in [
        ("x.cs", Language::CSharp),
        ("x.ts", Language::TypeScript),
        ("x.tsx", Language::Tsx),
        ("x.js", Language::JavaScript),
        ("x.py", Language::Python),
        ("x.rs", Language::Rust),
    ] {
        let f = extract_file(path, lang, "class { def ( => {{ \"unterminated \n } } ]]] @@@");
        assert_eq!(f.symbols[0].kind, SymbolKind::Module);
    }
}

#[test]
fn csharp_expression_bodied_property_is_walked_once() {
    let f = extract_file("P.cs", Language::CSharp, "class P { string T => \"edge-title\"; }");
    assert_eq!(f.literals.iter().filter(|l| l.value == "edge-title").count(), 1, "{:?}", f.literals);
}
