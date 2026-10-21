//! Functions nested in functions, route-registering helpers, environment aliases and deep
//! class hierarchies (fixtures/edge-cases/closures).

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/closures")
}

fn routes(c: &rusqlite::Connection) -> Vec<String> {
    let mut st = c.prepare("SELECT http_method || ' ' || path || ' ' || framework FROM api_route ORDER BY 1").unwrap();
    st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
}

fn config_reads(c: &rusqlite::Connection) -> Vec<String> {
    let mut st = c.prepare("SELECT DISTINCT config_key FROM config_dependency WHERE usage = 'read' ORDER BY 1").unwrap();
    st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
}

#[test]
fn nested_functions_are_symbols_with_their_own_calls() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "buildApp.actor").kind, "function");
    assert_eq!(sym(c, "buildApp.route").kind, "function", "arrow function bound to a const");
    // Calls inside the nested function (and its inline callbacks) belong to it.
    assert_eq!(edge(c, "buildApp.route", "BaseService.rate", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "buildApp.route", "CALLS"));
    assert_eq!(edge(c, "buildApp.route", "buildApp.actor", "CALLS").as_deref(), Some("exact"), "local function call");
    assert_eq!(edge(c, "create_app.profile", "create_app.load_user", "CALLS").as_deref(), Some("exact"), "python closure");
}

#[test]
fn route_helpers_register_routes() {
    let f = fixture();
    let c = &f.conn;
    let r = routes(c);
    for want in ["GET /v1/me fastify", "POST /v1/friends fastify", "GET /health fastify", "GET /orders express", "GET /profile flask"] {
        assert!(r.iter().any(|x| x == want), "{want} missing: {r:?}");
    }
    // The route handler reaches the service, and the helper's own request logic.
    assert_eq!(edge(c, "GET /v1/me", "AppService.me", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "GET /v1/me", "CALLS"));
    assert!(edge(c, "GET /v1/me", "buildApp.route", "CALLS").is_some(), "{:?}", targets(c, "GET /v1/me", "CALLS"));
    assert!(edge(c, "GET /orders", "listOrders", "CALLS").is_some(), "{:?}", targets(c, "GET /orders", "CALLS"));
}

#[test]
fn environment_aliases_and_accessors_are_config_reads() {
    let f = fixture();
    let c = &f.conn;
    let keys = config_reads(c);
    for want in ["DATABASE_URL", "PORT", "WEB_ORIGINS", "HEALTH_URL"] {
        assert!(keys.iter().any(|k| k == want), "{want} missing: {keys:?}");
    }
    for unwanted in ["someField", "SHOULD_NOT_COUNT", "name"] {
        assert!(!keys.iter().any(|k| k == unwanted), "{unwanted} is not an environment read: {keys:?}");
    }
}

#[test]
fn deep_hierarchies_resolve_inherited_members_and_every_call_site() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "AppService.me", "BaseService.rate", "CALLS").as_deref(), Some("exact"), "8 levels of inheritance");
    let sites = count(
        c,
        "SELECT COUNT(DISTINCT e.line_number) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = 'CALLS' AND s.qualified_name = 'AppService.me' AND t.qualified_name = 'BaseService.rate'",
    );
    assert_eq!(sites, 2);
}

#[test]
fn http_client_wrappers_link_callers_to_routes() {
    let f = fixture();
    let c = &f.conn;
    // addFriend → protectedRequest(…, '/v1/friends') → request('POST', path) → fetch(base + path).
    assert!(edge(c, "addFriend", "POST /v1/friends", "HTTP_CALLS").is_some(), "{:?}", targets(c, "addFriend", "HTTP_CALLS"));
    assert!(edge(c, "me", "GET /v1/me", "HTTP_CALLS").is_some(), "{:?}", targets(c, "me", "HTTP_CALLS"));
    // The wrappers themselves call no route.
    assert!(targets(c, "request", "HTTP_CALLS").is_empty(), "{:?}", targets(c, "request", "HTTP_CALLS"));
}

#[test]
fn framework_error_handlers_and_interface_families() {
    let f = fixture();
    let c = &f.conn;
    let caught = count(c, "SELECT COUNT(*) FROM edge e JOIN node t ON t.id = e.target_id WHERE e.relation = 'CAUGHT_BY' AND t.name = 'error handler'");
    assert!(caught > 0, "setErrorHandler catches what routes throw");
    // `tx` is untyped in the callback, but every `.one()` implements `Repo.one`.
    assert_eq!(edge(c, "AppService.friend", "Repo.one", "CALLS").as_deref(), Some("inferred"), "{:?}", targets(c, "AppService.friend", "CALLS"));
}
