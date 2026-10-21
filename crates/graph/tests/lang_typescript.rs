//! TypeScript / JavaScript language edge cases (fixtures/edge-cases/typescript).

mod common;

use common::*;

fn fixture() -> Fixture {
    index_fixture("edge-cases/typescript")
}

fn file_of(f: &Fixture, qname: &str) -> String {
    sym(&f.conn, qname).file
}

#[test]
fn tsconfig_paths_base_url_and_workspace_packages() {
    let f = fixture();
    let c = &f.conn;
    // `@app/services/users` via paths, `this.users` typed by a parameter property.
    assert_eq!(edge(c, "Controller2.load", "UsersService.find", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "Controller2.load", "shared", "CALLS").as_deref(), Some("exact"), "@shared exact path");
    assert_eq!(edge(c, "Controller2.load", "utilFn", "CALLS").as_deref(), Some("exact"), "@acme/util workspace package");
    assert_eq!(sym(c, "Controller2.users").kind, "field", "constructor parameter property");
}

#[test]
fn re_exports_aliases_and_lazy_imports() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "Controller2.load", "sq", "CALLS").is_some(), "export * as math: {:?}", targets(c, "Controller2.load", "CALLS"));
    assert!(edge(c, "Controller2.load", "inner", "CALLS").is_some(), "export {{ inner as outer }}");
    let secrets = target_sigs(c, "Controller2.load", "CALLS");
    let lazy_secret = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id \
         WHERE e.relation = 'CALLS' AND s.qualified_name = 'Controller2.load' AND t.name = 'secret' AND t.file_path = 'src/app/admin.ts'",
    );
    assert_eq!(lazy_secret, 0, "dynamic import is not a re-export: {secrets:?}");
}

#[test]
fn generics_return_types_and_super() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "save", "Entity.persist", "CALLS").is_some(), "T extends Entity");
    assert!(edge(c, "useReturn", "UsersService.find", "CALLS").is_some(), "const svc = getSvc(): declared return type");
    let sup = target_sigs(c, "Kid.constructor", "CALLS");
    assert!(sup.iter().any(|(k, s)| k == "constructor" && s.contains("a: number")), "super(1): {sup:?}");
}

#[test]
fn locals_shadow_module_functions() {
    let f = fixture();
    let c = &f.conn;
    // `const helper = () => 1` inside `shadow` is the callee, not the module-level `helper`.
    let module_level = count(
        c,
        "SELECT COUNT(*) FROM edge e JOIN node s ON s.id = e.source_id JOIN node t ON t.id = e.target_id          WHERE e.relation = 'CALLS' AND s.qualified_name = 'shadow' AND t.qualified_name = 'helper'",
    );
    assert_eq!(module_level, 0, "{:?}", targets(c, "shadow", "CALLS"));
    assert_eq!(edge(c, "shadow", "shadow.helper", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "shadow", "CALLS"));
}

#[test]
fn types_decorators_enums_and_classes_as_values() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "show", "OrderDto", "USES_TYPE").is_some(), "parameter annotation");
    assert!(edge(c, "show", "Status.Active", "READS_VAR").is_some(), "enum member read");
    assert!(edge(c, "show", "UsersService", "USES_TYPE").is_some(), "class used as a value");
    assert!(
        edge(c, "Controller2", "Roles", "CALLS").is_some() || edge(c, "Controller2.load", "Roles", "CALLS").is_some(),
        "decorator call"
    );
    assert!(sym(c, "Columns.id").attrs.contains("Column()"), "field decorator kept: {:?}", sym(c, "Columns.id"));
}

#[test]
fn namespaces_overloads_private_fields_and_accessors() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "callNamespace", "Util.Inner.f", "CALLS").as_deref(), Some("exact"));
    let m = syms(c, "Overloaded.m");
    assert_eq!(m.len(), 1, "overload signatures fold into the implementation: {m:?}");
    assert_eq!(target_sigs(c, "useOverloaded", "CALLS").len(), 1, "no dispatch from overloads");
    assert!(edge(c, "Counter.inc", "Counter.#count", "WRITES_VAR").is_some(), "#private: {:?}", targets(c, "Counter.inc", "WRITES_VAR"));
    let total = syms(c, "Cart.total");
    assert_eq!(total.len(), 1, "get + set = one property: {total:?}");
    assert_eq!(total[0].kind, "property");
    assert!(edge(c, "readTotal", "Cart.total", "READS_VAR").is_some(), "getter read");
}

#[test]
fn object_literals_class_expressions_and_default_classes() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "api.list").kind, "method");
    assert!(edge(c, "useApi", "api.list", "CALLS").is_some());
    assert_eq!(sym(c, "Klass").kind, "class", "const Klass = class {{}}");
    assert!(syms(c, "default.go").iter().any(|s| s.kind == "method"), "export default class {{ go() }}");
}

#[test]
fn commonjs_modules() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "sub").kind, "function", "exports.sub = …");
    assert!(edge(c, "total", "add", "CALLS").is_some(), "destructured require: {:?}", targets(c, "total", "CALLS"));
    assert!(edge(c, "total", "sub", "CALLS").is_some(), "require inside a function: {:?}", targets(c, "total", "CALLS"));
    assert!(edge(c, "legacy", "add", "CALLS").is_some(), "import x = require()");
}

#[test]
fn react_wrappers_and_hocs() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(sym(c, "Row").kind, "function", "memo(…) component");
    assert!(edge(c, "Page", "Row", "CALLS").is_some(), "<Row/> resolves to the memo component");
    assert!(edge(c, "Shell", "Page", "CALLS").is_some(), "default export through withRouter(Page)");
}

#[test]
fn nextjs_tests_and_routes() {
    let f = fixture();
    let c = &f.conn;
    let get = sym(c, "GET");
    assert_eq!(get.entry_kind, "route GET /api/users", "{get:?}");
    let page = sym(c, "DashboardPage");
    assert!(page.is_entrypoint && page.entry_kind.contains("/dashboard"), "{page:?}");
    assert!(syms(c, "adds %i").iter().any(|s| s.is_test), "it.each");
    assert!(count(c, "SELECT COUNT(*) FROM node WHERE kind = 'test' AND name = 'UsersService.name'") == 1, "describe(X.name)");
    let routes: Vec<String> = {
        let mut st = c.prepare("SELECT http_method || ' ' || path FROM api_route ORDER BY 1").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
    };
    for r in ["GET /items", "GET /health", "GET /cats/all", "PAGE /dashboard"] {
        assert!(routes.iter().any(|x| x == r), "{r} missing: {routes:?}");
    }
    assert_eq!(count(c, "SELECT COUNT(*) FROM file_meta WHERE file_path LIKE '%.d.mts'"), 0, ".d.mts is a declaration file");
    let _ = file_of(&f, "save");
}

#[test]
fn exports_listed_before_their_declarations() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "useHoisted", "early", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "useHoisted", "CALLS"));
    assert!(edge(c, "useHoisted", "other", "CALLS").is_some(), "export {{ other as renamed }}: {:?}", targets(c, "useHoisted", "CALLS"));
    assert!(edge(c, "useHoisted", "Late.run", "CALLS").is_some(), "export default Late: {:?}", targets(c, "useHoisted", "CALLS"));
}

#[test]
fn array_receivers_and_element_types() {
    let f = fixture();
    let c = &f.conn;
    assert!(edge(c, "pickMembers", "Member.find", "CALLS").is_none(), "members.find is Array.prototype.find");
    assert_eq!(edge(c, "pickMembers", "Member.ship", "CALLS").as_deref(), Some("exact"), "members.map(m => m.ship())");
    assert_eq!(edge(c, "pickMembers", "Member.enroll", "CALLS").as_deref(), Some("exact"), "members.find(m => m.enroll())");
    assert_eq!(edge(c, "Roster.each", "Member.ship", "CALLS").as_deref(), Some("exact"), "this.members.forEach(m => …)");
    assert_eq!(edge(c, "Roster.each", "Member.enroll", "CALLS").as_deref(), Some("exact"), "for (const m of this.members)");
}

#[test]
fn unannotated_return_types() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "runEngine", "Engine.ignite", "CALLS").as_deref(), Some("exact"), "{:?}", targets(c, "runEngine", "CALLS"));
}

#[test]
fn jsdoc_types_and_prototype_methods() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "book", "Ledger.post", "CALLS").as_deref(), Some("exact"), "@param {{Ledger}}");
    assert_eq!(edge(c, "flush", "Ledger.post", "CALLS").as_deref(), Some("exact"), "@type {{Ledger}}");
    assert_eq!(sym(c, "Account.close").kind, "method");
    assert_eq!(edge(c, "Account.close", "Account.audit", "CALLS").as_deref(), Some("exact"), "this.audit() in a prototype method");
}

#[test]
fn express_router_mounts() {
    let f = fixture();
    let c = &f.conn;
    let routes: Vec<String> = {
        let mut st = c.prepare("SELECT http_method || ' ' || path FROM api_route ORDER BY 1").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
    };
    for r in ["GET /api/v2/status", "GET /api/members/:id", "GET /items"] {
        assert!(routes.iter().any(|x| x == r), "{r} missing: {routes:?}");
    }
    assert!(!routes.iter().any(|x| x == "GET /status" || x == "GET /:id"), "mounted routes keep only their full path: {routes:?}");
}

#[test]
fn vue_and_svelte_components() {
    let f = fixture();
    let c = &f.conn;
    let dash = sym(c, "Dashboard");
    assert!(dash.file.ends_with("Dashboard.vue") && dash.attrs.contains("export default"), "{dash:?}");
    assert_eq!(edge(c, "Dashboard", "UserCard", "CALLS").as_deref(), Some("exact"), "<UserCard> / <user-card> in the template");
    assert_eq!(edge(c, "Dashboard", "refresh", "CALLS").as_deref(), Some("exact"), "@click=\"refresh\"");
    assert_eq!(edge(c, "Dashboard", "formatTotal", "CALLS").as_deref(), Some("exact"), "{{{{ formatTotal(total) }}}}");
    assert!(edge(c, "Dashboard", "ready", "READS_VAR").is_some(), "v-if=\"ready\"");
    assert!(callers(c, "unused", "CALLS").is_empty(), "only template uses count");
    // Options API: methods are members of the component, called from the template and `this`.
    assert_eq!(edge(c, "Options", "Options.save", "CALLS").as_deref(), Some("exact"));
    assert_eq!(edge(c, "Options.save", "Options.persist", "CALLS").as_deref(), Some("exact"));
    assert_eq!(sym(c, "Options.mounted").entry_kind, "vue lifecycle");
    // Svelte markup.
    assert_eq!(edge(c, "Counter", "increment", "CALLS").as_deref(), Some("exact"), "on:click={{increment}}");
    assert!(syms(c, "Counter").iter().any(|s| s.file.ends_with("Counter.svelte")));
    assert_eq!(edge(c, "Counter", "UserCard", "CALLS").as_deref(), Some("exact"));
}

#[test]
fn lazy_components_super_calls_and_package_exports() {
    let f = fixture();
    let c = &f.conn;
    assert_eq!(edge(c, "Shell2", "Panel", "CALLS").as_deref(), Some("exact"), "lazy(() => import('./Panel'))");
    assert_eq!(edge(c, "Dog.speak", "Animal.speak", "CALLS").as_deref(), Some("exact"), "super.speak()");
    assert_eq!(edge(c, "useFmt", "fmtFn", "CALLS").as_deref(), Some("exact"), "package.json exports subpath");
}
