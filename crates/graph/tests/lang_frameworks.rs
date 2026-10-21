//! Framework support across a Vite + React frontend, a Fastify API and Playwright e2e tests
//! (fixtures/edge-cases/frameworks).

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/frameworks")
}

fn routes(c: &rusqlite::Connection) -> Vec<String> {
    let mut st = c.prepare("SELECT http_method || ' ' || path || ' ' || framework FROM api_route ORDER BY 1").unwrap();
    st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
}

#[test]
fn vite_aliases_proxies_and_globs() {
    let f = fixture();
    let c = &f.conn;
    // `#app/…` resolves through `resolve.alias` in web/vite.config.ts.
    assert_eq!(edge(c, "router", "UsersPage", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "router", "CALLS"));
    // fetch('/api/users') reaches the Fastify route through the dev-server proxy rewrite.
    assert!(edge(c, "listUsers", "GET /", "HTTP_CALLS").is_some(), "{:?}", targets(c, "listUsers", "HTTP_CALLS"));
    // import.meta.glob('./*.tsx') loads Home.
    let globbed = count(c, "SELECT COUNT(*) FROM edge e JOIN node t ON t.id = e.target_id WHERE e.relation = 'IMPORTS' AND t.name = 'Home'");
    assert_eq!(globbed, 1);
}

#[test]
fn react_routes_context_props_and_state() {
    let f = fixture();
    let c = &f.conn;
    let r = routes(c);
    for want in ["PAGE /users react-router", "PAGE /users/:id react-router", "PAGE /account/settings react-router"] {
        assert!(r.iter().any(|x| x == want), "{want} missing: {r:?}");
    }
    assert_eq!(edge(c, "UsersPage", "Session.refresh", "CALLS").as_deref(), Some("exact"), "useContext(AuthContext)");
    assert_eq!(edge(c, "UserDetail", "Session.logout", "CALLS").as_deref(), Some("exact"), "({{ session }}: DetailProps)");
    assert_eq!(sym(c, "DetailProps.session").kind, "property", "type alias members");
    assert_eq!(sym(c, "useCart.add").kind, "method", "zustand store member");
    let muts = count(
        c,
        "SELECT COUNT(*) FROM state_mutation WHERE (target_state = 'count' AND mutation_type = 'component_state') \
         OR (target_state = 'useCart' AND mutation_type = 'store_state')",
    );
    assert_eq!(muts, 2, "setCount(…) and set(…) inside the store");
}

#[test]
fn fastify_autoload_hooks_decorations_and_inject() {
    let f = fixture();
    let c = &f.conn;
    assert!(routes(c).iter().any(|x| x == "GET /users fastify"), "autoload folder prefix: {:?}", routes(c));
    assert!(edge(c, "GET /", "Db.findUsers", "CALLS").is_some(), "fastify.db via decorate('db', new Db())");
    assert!(edge(c, "GET /", "authenticate", "CALLS").is_some(), "decorated function");
    assert_eq!(sym(c, "hook onRequest").entry_kind, "fastify hook onRequest");
    assert_eq!(edge(c, "hook onRequest", "audit", "CALLS").as_deref(), Some("exact"));
    assert!(edge(c, "GET /users", "GET /", "TESTS").is_some(), "app.inject({{ url: '/users' }}) tests the route");
}

#[test]
fn playwright_fixtures_pages_and_test_ids() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "lists users", "UsersPom.open", "CALLS").as_deref(), Some("exact"), "fixture typed by base.extend<Fixtures>");
    assert!(callers(c, "UsersPom.open", "CALLS").iter().any(|s| s.ends_with("users.spec")), "test.beforeEach(({{ usersPage }}) => …)");
    assert!(edge(c, "lists users", "UsersPage", "HTTP_CALLS").is_some(), "page.goto('/users')");
    assert!(edge(c, "lists users", "UsersPage", "TESTS").is_some(), "visited page / getByTestId('refresh-users')");
    assert!(edge(c, "lists users", "Settings", "TESTS").is_some(), "locator('[data-testid=\"settings-form\"]')");
}
