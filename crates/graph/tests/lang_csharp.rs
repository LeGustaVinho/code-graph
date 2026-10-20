//! C# language edge cases (fixtures/edge-cases/csharp): every assertion names the construct
//! it protects, so a regression points straight at the feature.

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/csharp")
}

#[test]
fn null_conditional_calls_keep_their_receiver() {
    let f = fixture();
    let c = &f.conn;
    // `_logger?.Log("x")` goes to the interface method, never to the same-class decoy.
    assert!(edge(c, "Caller.Run", "ILogger2.Log", "CALLS").is_some(), "{:?}", targets(c, "Caller.Run", "CALLS"));
    assert!(edge(c, "Caller.Run", "Caller.Log", "CALLS").is_none(), "{:?}", targets(c, "Caller.Run", "CALLS"));
    // `FindUser(1)?.Activate()`: the receiver call is walked and the member is linked.
    assert!(edge(c, "Caller.Run", "Caller.FindUser", "CALLS").is_some());
    assert!(edge(c, "Caller.Run", "Svc.Activate", "CALLS").is_some());
}

#[test]
fn tuple_deconstruction_walks_the_initializer() {
    let f = fixture();
    assert!(edge(&f.conn, "Caller.Run", "Caller.Compute", "CALLS").is_some());
}

#[test]
fn nameof_is_not_a_call() {
    let f = fixture();
    assert!(edge(&f.conn, "Caller.Run", "Caller.Process", "CALLS").is_none());
}

#[test]
fn object_initializer_writes_the_created_type() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Caller.Run", "Edge.Calls.Order.Name", "WRITES_VAR").is_some(), "{:?}", targets(c, "Caller.Run", "WRITES_VAR"));
    assert!(edge(c, "Caller.Run", "Caller.Name", "WRITES_VAR").is_none());
}

#[test]
fn foreach_variable_is_typed() {
    let f = fixture();
    assert_eq!(edge(&f.conn, "Caller.Run", "Order.Ship", "CALLS").as_deref(), Some("exact"));
}

#[test]
fn params_arrays_count_for_overload_resolution() {
    let f = fixture();
    let c = &f.conn;
    let write = syms(c, "Params.Write");
    let variadic = write.iter().find(|s| s.signature.contains("params")).expect("params overload");
    assert!(variadic.params.contains("\"variadic\":true"), "{}", variadic.params);
    // `Write("{0} {1}", 1, 2)` only fits the params overload.
    let sigs = target_sigs(c, "Params.Use", "CALLS");
    assert_eq!(sigs.len(), 1, "{sigs:?}");
    assert!(sigs[0].1.contains("params"), "{sigs:?}");
    // An attribute argument with `=` does not make a parameter optional.
    let read = sym(c, "Params.Read");
    assert!(read.params.contains("\"required\":1"), "{}", read.params);
}

#[test]
fn constructors_skip_static_initialisers() {
    let f = fixture();
    let c = &f.conn;
    let make = target_sigs(c, "Factory.Make", "CALLS");
    assert!(make.iter().any(|(k, s)| k == "constructor" && s.contains("int size")), "{make:?}");
    assert!(!make.iter().any(|(k, s)| k == "constructor" && !s.contains("size")), "static ctor targeted: {make:?}");
    let ms = target_sigs(c, "Factory.MakeStatic", "CALLS");
    assert!(ms.iter().all(|(k, _)| k == "class"), "implicit ctor → the type, not the static ctor: {ms:?}");
}

#[test]
fn record_struct_is_a_struct() {
    let f = fixture();
    assert_eq!(sym(&f.conn, "Edge.Calls.Point").kind, "struct");
}

#[test]
fn nested_block_namespaces_compose() {
    let f = fixture();
    let deep = sym(&f.conn, "Deep");
    assert_eq!(deep.qualified_name, "Edge.Outer.Inner.Deep");
}

#[test]
fn every_namespace_block_of_a_file_is_visible() {
    let f = fixture();
    // Gamma exists in two namespaces; Beta (second block of its file) sees Edge.Second.
    assert_eq!(edge(&f.conn, "Beta.Go", "Edge.Second.Gamma.Pong", "CALLS").as_deref(), Some("exact"));
    assert!(edge(&f.conn, "Beta.Go", "Edge.Third.Gamma.Pong", "CALLS").is_none());
}

#[test]
fn file_names_ending_in_test_are_not_tests() {
    let f = fixture();
    let test_modules = count(&f.conn, "SELECT COUNT(*) FROM node WHERE kind = 'module' AND file_path = 'Latest.cs' AND is_test = 1");
    assert_eq!(test_modules, 0, "Latest.cs is production code");
    assert!(codegraph_core::extract::is_test_path("tests/OrderServiceTests.cs"));
    assert!(codegraph_core::extract::is_test_path("src/OrderServiceTest.cs"));
    assert!(!codegraph_core::extract::is_test_path("src/Contest.cs"));
}
