//! Regression cases for lexically scoped TS wrappers, JS/Python closures and Rust calls.

mod common;
use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/closures")
}

#[test]
fn route_and_environment_helpers_do_not_escape_their_lexical_scope() {
    let f = fixture();
    let c = &f.conn;
    let routes = count(c, "SELECT COUNT(*) FROM api_route WHERE path = '/genuine'");
    let leaked_routes = count(c, "SELECT COUNT(*) FROM api_route WHERE path = '/phantom'");
    assert_eq!(routes, 1, "route helper in its declaring function must be recognized");
    assert_eq!(leaked_routes, 0, "an unrelated helper with the same name is not a router");

    let real_key = count(c, "SELECT COUNT(*) FROM config_dependency WHERE config_key = 'GENUINE_KEY' AND usage = 'read'");
    let leaked_key = count(c, "SELECT COUNT(*) FROM config_dependency WHERE config_key = 'NOT_AN_ENV_KEY' AND usage = 'read'");
    assert!(real_key > 0, "accessor reading a process environment alias should be recognized");
    assert_eq!(leaked_key, 0, "an unrelated same-named accessor is not an environment reader");
}

#[test]
fn nested_functions_and_repeated_calls_in_three_grammars() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "outer.emit").kind, "function");
    assert_eq!(sym(c, "outer.twice").kind, "function");
    assert_eq!(sym(c, "outer.caller").kind, "function");
    assert_eq!(edge(c, "outer.twice", "outer.emit", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "outer.caller", "outer.target", "CALLS").as_deref(), Some("exact"));
    let js_sites = count(c, "SELECT COUNT(DISTINCT line_number) FROM edge e JOIN node s ON s.id=e.source_id JOIN node t ON t.id=e.target_id WHERE e.relation='CALLS' AND s.qualified_name='outer.twice' AND t.qualified_name='outer.emit'");
    assert_eq!(js_sites, 2, "both statements in a JS callback must be indexed");
    assert_eq!(sym(c, "invoke").kind, "function");
}
