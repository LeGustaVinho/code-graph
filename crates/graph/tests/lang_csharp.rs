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

// ---------------------------------------------------------------- phase 2: gaps

#[test]
fn every_preprocessor_branch_produces_edges() {
    let f = fixture();
    let c = &f.conn;
    for target in ["StoreReview.AskIos", "StoreReview.AskAndroid", "StoreReview.AskEditor"] {
        assert!(edge(c, "StoreReview.Ask", target, "CALLS").is_some(), "{target}: {:?}", targets(c, "StoreReview.Ask", "CALLS"));
    }
}

#[test]
fn extension_methods_resolve_on_typed_receivers() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "Usage.Run", "OrderExtensions.ToDto", "CALLS").as_deref(), Some("exact"));
    // Extension on an interface the receiver is typed with.
    assert_eq!(edge(c, "Usage.Run", "OrderExtensions.AddRepos", "CALLS").as_deref(), Some("exact"));
}

#[test]
fn generic_constraints_type_receivers() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Repo.Touch", "Entity.Save", "CALLS").is_some(), "class constraint");
    assert!(edge(c, "Repo.Each", "Entity.Save", "CALLS").is_some(), "method constraint");
    assert!(sym(c, "Edge.Gaps.Repo").signature.contains("Repo<T>"), "{}", sym(c, "Edge.Gaps.Repo").signature);
}

#[test]
fn collection_elements_are_typed() {
    let f = fixture();
    let c = &f.conn;
    let calls = targets(c, "Usage.Run", "CALLS");
    assert!(calls.iter().any(|t| t.ends_with("Order.Ship")), "foreach var: {calls:?}");
    assert!(calls.iter().any(|t| t.ends_with("Entity.Save")), "orders[0]: {calls:?}");
    assert!(calls.iter().any(|t| t.ends_with("Order.IsPaid")), "LINQ lambda: {calls:?}");
}

#[test]
fn var_types_follow_declared_return_types() {
    let f = fixture();
    let c = &f.conn;
    // `var o = _repo.Find(1); o.Ship();`, `var cast = y as Order;`, `var loaded = await _repo.LoadAsync();`
    assert_eq!(edge(c, "Usage.Run", "Order.Ship", "CALLS").as_deref(), Some("exact"));
    let ship_calls = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = 'CALLS' AND s.qualified_name = 'Edge.Gaps.Usage.Run' AND t.qualified_name = 'Edge.Gaps.Order.Ship'",
    );
    assert!(ship_calls >= 4, "Ship() via foreach, var, as and await: {ship_calls}");
}

#[test]
fn member_reads_reach_properties_constants_and_records() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Usage.Run", "Order.Total", "READS_VAR").is_some(), "{:?}", targets(c, "Usage.Run", "READS_VAR"));
    assert!(edge(c, "Usage.Run", "Color.Red", "READS_VAR").is_some(), "enum member");
    assert_eq!(sym(c, "Edge.Gaps.Person.Name").kind, "property", "positional record parameter");
    assert!(edge(c, "Usage.Run", "Person.Name", "READS_VAR").is_some(), "record property read");
}

#[test]
fn constructor_chaining_and_target_typed_new() {
    let f = fixture();
    let c = &f.conn;
    let base = target_sigs(c, "Derived2.Derived2", "CALLS");
    assert!(base.iter().any(|(k, s)| k == "constructor" && s.contains("int a")), "base(1): {base:?}");
    assert!(base.iter().any(|(k, s)| k == "constructor" && s.starts_with("Derived2()")), "this(): {base:?}");
    let primary = target_sigs(c, "Edge.Gaps.Primary", "CALLS");
    assert!(primary.iter().any(|(k, s)| k == "constructor" && s.contains("int a")), "primary ctor base: {primary:?}");
    assert!(edge(c, "Usage.Run", "Edge.Gaps.Order", "CALLS").is_some(), "Order created = new();");
}

#[test]
fn attributes_and_typeof_are_type_uses() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Edge.Gaps.Tagged", "MarkerAttribute", "USES_TYPE").is_some());
    assert!(edge(c, "Usage.Run", "Edge.Gaps.Dto", "USES_TYPE").is_some());
}

#[test]
fn partial_classes_are_one_type() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "Big.A", "Big.B", "CALLS").as_deref(), Some("exact"), "implicit this across parts");
    let make = target_sigs(c, "BigUser.Make", "CALLS");
    assert!(make.iter().any(|(k, s)| k == "constructor" && s.contains("int size")), "new Big(3): {make:?}");
    let unresolved = count(c, "SELECT COUNT(*) FROM edge WHERE confidence = 'unresolved' AND unresolved_symbol = 'Big'");
    assert_eq!(unresolved, 0, "partial parts are not ambiguous candidates");
}

#[test]
fn operators_events_delegates_and_locals_are_symbols() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Money.operator +", "Money.Add", "CALLS").is_some(), "operator body has its own scope");
    assert!(edge(c, "Events.Changed", "Events.Hook", "CALLS").is_some(), "event accessor bodies");
    assert_eq!(sym(c, "Edge.Gaps.Handler").kind, "type_alias");
    let compute = target_sigs(c, "Locals.Compute", "CALLS");
    assert!(compute.iter().any(|(k, _)| k == "function"), "local function wins: {compute:?}");
    assert!(!compute.iter().any(|(k, _)| k == "method"), "not the same-named method: {compute:?}");
}

#[test]
fn virtual_calls_reach_overrides_and_explicit_implementations_dispatch() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "Zoo.Play", "Animal.Speak", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "Zoo.Play", "Dog.Speak", "CALLS").as_deref(), Some("inferred"));
    assert!(edge(c, "StoreUser.Use", "Store.IStore.Persist", "CALLS").is_some(), "{:?}", targets(c, "StoreUser.Use", "CALLS"));
    assert!(sym(c, "Store.IStore.Persist").params.len() < 200);
}

#[test]
fn usings_static_alias_and_global() {
    let f = fixture();
    let c = &f.conn;
    let calls = target_sigs(c, "Usage.Run", "CALLS");
    let clamps = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = 'CALLS' AND s.qualified_name = 'Edge.Gaps.Usage.Run' AND t.qualified_name = 'Edge.Gaps.MathUtil.Clamp2'",
    );
    assert!(clamps >= 2, "using static + alias: {clamps} {calls:?}");
    assert_eq!(edge(c, "Consumer.Go", "Edge.Lib.LibThing.Work", "CALLS").as_deref(), Some("exact"));
    assert!(edge(c, "Consumer.Go", "Edge.Other.LibThing.Work", "CALLS").is_none());
}

#[test]
fn generated_files_and_test_hooks() {
    let f = fixture();
    let c = &f.conn;
    let gen = count(c, "SELECT COUNT(*) FROM node WHERE kind = 'module' AND file_path = 'Model.g.cs' AND attrs LIKE '%generated%'");
    assert_eq!(gen, 1);
    let init = sym(c, "HookTests.Init");
    assert!(init.is_entrypoint && init.entry_kind.contains("SetUp"), "{init:?}");
    assert!(sym(c, "HookTests.Plays").is_test);
}
