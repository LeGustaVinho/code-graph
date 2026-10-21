//! TypeScript / TSX / JavaScript extractor.

use tree_sitter::Node;

use super::*;
use crate::routes::join_route;

const SERVER_PACKAGES: &[&str] = &["express", "fastify", "fastify-plugin", "koa", "@koa/router", "koa-router", "hono", "restify"];

/// Route framework of a server package (`fastify-plugin` files declare Fastify routes).
fn server_framework(module: &str) -> &'static str {
    match module {
        "fastify" | "fastify-plugin" => "fastify",
        "koa" | "@koa/router" | "koa-router" => "koa",
        "hono" => "hono",
        "restify" => "restify",
        _ => "express",
    }
}
const TEST_FNS: &[&str] = &["describe", "it", "test", "suite", "context", "specify"];
const HTTP_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];
const BUILTIN_IDENTS: &[&str] = &[
    "undefined",
    "NaN",
    "Infinity",
    "arguments",
    "require",
    "module",
    "exports",
    "console",
    "window",
    "document",
    "globalThis",
    "JSON",
    "Math",
    "Object",
    "Array",
    "Promise",
    "String",
    "Number",
    "Boolean",
    "Date",
    "Error",
    "Symbol",
    "Map",
    "Set",
    "process",
    "this",
    "super",
    "null",
    "true",
    "false",
];

/// Library / language types never worth a USES_TYPE lookup.
const TS_BUILTIN_TYPES: &[&str] = &[
    "string",
    "number",
    "boolean",
    "any",
    "unknown",
    "never",
    "void",
    "object",
    "bigint",
    "symbol",
    "Promise",
    "Array",
    "ReadonlyArray",
    "Record",
    "Partial",
    "Readonly",
    "Required",
    "Pick",
    "Omit",
    "Exclude",
    "Extract",
    "NonNullable",
    "ReturnType",
    "Parameters",
    "InstanceType",
    "Awaited",
    "Map",
    "Set",
    "WeakMap",
    "WeakSet",
    "Date",
    "Error",
    "Function",
    "RegExp",
    "Iterable",
    "AsyncIterable",
    "Iterator",
    "Generator",
    "JSX",
    "React",
    "Element",
    "HTMLElement",
    "Event",
];

/// Component wrappers whose first argument is the component itself.
const COMPONENT_WRAPPERS: &[&str] = &["memo", "forwardRef", "observer", "styled", "withStyles"];

#[derive(Default)]
struct State {
    server_file: bool,
    /// Framework of the server package this file imports (`express`, `fastify`, …).
    framework: Option<&'static str>,
    has_exports: bool,
    /// `export { a }` / `export default A` / `module.exports = { a }` may precede the
    /// (hoisted) declarations they name: applied once the whole file has been visited.
    exported: Vec<(String, bool)>,
    /// `export { a as b }`: (local name, exported alias).
    aliases: Vec<(String, String)>,
    /// Server routes (index into `facts.routes`) → the router variable they are declared on.
    route_recv: Vec<(usize, String)>,
    /// `app.use('/api', router)` mounts, applied once the file is done.
    mounts: Vec<Mount>,
    /// Handler nodes (by id) whose bodies were already walked as route handlers.
    walked_handlers: Vec<usize>,
    /// React `useState` / `useReducer` setters → the state they update.
    setters: HashMap<String, String>,
    /// Zustand / Pinia / Redux store whose members are being visited (`set(…)` mutates it).
    store: Option<String>,
    /// A Playwright test file (`@playwright/test` or a local `test` fixture module).
    playwright: bool,
    /// A Phaser file (imports `phaser` or uses the `Phaser` global / a scene's `this.load`).
    phaser: bool,
}

/// Attributes Playwright / Cypress / Testing Library locate elements by.
pub(crate) const TESTID_ATTRS: &[&str] = &["data-testid", "data-test-id", "data-test", "data-cy", "data-qa"];

struct Mount {
    prefix: String,
    /// Router variable, or `module:<spec>` for `app.use('/x', require('./x'))`.
    target: String,
    /// Router / app the mount is declared on.
    parent: String,
}

#[derive(Clone, Default)]
struct ClassInfo {
    route_prefix: Option<String>,
    is_controller: bool,
    /// The class extends something: its `create` / `update` / … may be engine hooks.
    has_bases: bool,
}

fn named_children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}

fn children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.children(&mut c).collect()
}

pub(crate) fn extract(ctx: &mut Ctx, root: Node) {
    let phaser = ctx.src.contains("Phaser.")
        || ctx.src.contains("from 'phaser'")
        || ctx.src.contains("from \"phaser\"")
        || ctx.src.contains("this.load.")
        || ctx.src.contains("this.scene.start(");
    let mut st = State { phaser, ..Default::default() };
    for ch in named_children(root) {
        match ch.kind() {
            "import_statement" => {
                if let Some(src) = ch.child_by_field_name("source") {
                    let m = string_value(src, ctx.src);
                    if SERVER_PACKAGES.contains(&m.as_str()) {
                        st.server_file = true;
                        st.framework = st.framework.or(Some(server_framework(&m)));
                    }
                    // Playwright specs import `test` from the runner or from a fixtures module.
                    let imports_test = ctx.text(ch).contains("test") && (m.starts_with('.') || m.starts_with('@') || m.starts_with('~'));
                    if m == "@playwright/test" || (imports_test && is_test_path(ctx.path) && !m.contains("vitest") && !m.contains("jest")) {
                        st.playwright = st.playwright || m == "@playwright/test" || ctx.src.contains("page.");
                    }
                }
            }
            "export_statement" => st.has_exports = true,
            // `'use client'` / `'use server'` (React server components).
            "expression_statement" if ch.named_child(0).map(|e| e.kind() == "string").unwrap_or(false) => {
                let d = string_value(ch.named_child(0).unwrap(), ctx.src);
                if d == "use client" || d == "use server" {
                    ctx.facts.symbols[0].attrs.push(d);
                }
            }
            _ => {
                let t = ctx.text(ch);
                if let Some(p) = SERVER_PACKAGES.iter().find(|p| t.contains(&format!("require(\"{p}\")")) || t.contains(&format!("require('{p}')"))) {
                    st.server_file = true;
                    st.framework = st.framework.or(Some(server_framework(p)));
                }
                if t.contains("module.exports") || t.contains("exports.") {
                    st.has_exports = true;
                }
            }
        }
    }
    for ch in named_children(root) {
        visit_top(ctx, &mut st, ch, false, false, Vec::new());
    }
    for (name, default) in std::mem::take(&mut st.exported) {
        mark_exported(ctx, &name, default);
    }
    for (name, alias) in std::mem::take(&mut st.aliases) {
        export_alias(ctx, &name, &alias);
    }
    apply_mounts(ctx, &st);
    // `'use server'` modules: every exported function is a server action the client invokes.
    if ctx.facts.symbols[0].attrs.iter().any(|a| a == "use server") {
        let module_id = ctx.module_id();
        for s in ctx.facts.symbols.iter_mut().filter(|s| s.parent_id.as_deref() == Some(module_id.as_str())) {
            if s.kind == SymbolKind::Function && s.is_exported {
                s.is_entrypoint = true;
                s.entry_kind = Some("server action".into());
            }
        }
    }
}

fn defer_export(st: &mut State, name: &str, default: bool) {
    st.exported.push((name.to_string(), default));
}

fn defer_alias(st: &mut State, name: &str, alias: &str) {
    st.aliases.push((name.to_string(), alias.to_string()));
}

/// `<T extends Base, U>` → { T: Some(Base), U: None }.
fn ts_type_params(ctx: &Ctx, node: Node) -> HashMap<String, Option<String>> {
    let mut map = HashMap::new();
    if let Some(tp) = node.child_by_field_name("type_parameters") {
        for p in named_children(tp).into_iter().filter(|p| p.kind() == "type_parameter") {
            let Some(n) = p.child_by_field_name("name") else { continue };
            let bound = p.child_by_field_name("constraint").and_then(|c| c.named_child(0)).map(|t| ctx.text(t).to_string());
            map.insert(ctx.text(n).to_string(), bound);
        }
    }
    map
}

fn subst_t(map: &HashMap<String, Option<String>>, ty: Option<String>) -> Option<String> {
    let t = ty?;
    match map.get(&clean_type(&t)) {
        Some(bound) => bound.clone(),
        None => Some(t),
    }
}

/// Records every named type inside an annotation as a type use of the current scope.
fn type_uses(ctx: &mut Ctx, node: Option<Node>) {
    let Some(node) = node else { return };
    // Type parameters (`<T>`, `infer U`, `[K in keyof T]`) are not uses of project types.
    let mut seen: Vec<String> = local_type_params(ctx, node);
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        let name = match n.kind() {
            "type_identifier" => Some(ctx.text(n).to_string()),
            "nested_type_identifier" => Some(last_segment(ctx.text(n)).to_string()),
            _ => None,
        };
        match name {
            Some(t) => {
                if !TS_BUILTIN_TYPES.contains(&t.as_str()) && !seen.contains(&t) && !is_type_param(ctx, &t) {
                    ctx.push_ref(RefKind::TypeUse, &t, None, None, None, line_of(n), Some("type annotation".into()));
                    seen.push(t);
                }
            }
            None => stack.extend(named_children(n)),
        }
    }
}

fn is_type_param(ctx: &Ctx, name: &str) -> bool {
    ctx.scopes.iter().any(|s| s.tparams.contains_key(name))
}

/// Type variables introduced inside an annotation: `infer U`, mapped `[K in …]`.
fn local_type_params(ctx: &Ctx, node: Node) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        match n.kind() {
            "infer_type" => out.extend(named_children(n).into_iter().find(|c| c.kind() == "type_identifier").map(|c| ctx.text(c).to_string())),
            "mapped_type_clause" => out.extend(n.child_by_field_name("name").map(|c| ctx.text(c).to_string())),
            _ => {}
        }
        stack.extend(named_children(n));
    }
    out
}

/// Decorators reference the decorator function and any classes passed to it
/// (`@UseGuards(JwtGuard)`, `@Inject(TOKEN)`), under the current scope.
fn walk_decorator(ctx: &mut Ctx, st: &mut State, d: Node) {
    let Some(expr) = d.named_child(0) else { return };
    match expr.kind() {
        "call_expression" => {
            let f = expr.child_by_field_name("function");
            let name = f.map(|f| last_segment(ctx.text(f)).to_string()).unwrap_or_default();
            let args = expr.child_by_field_name("arguments");
            ctx.push_ref(RefKind::Call, &name, None, None, args.map(count_args), line_of(d), Some("decorator".into()));
            if let Some(a) = args {
                walk(ctx, st, a);
            }
        }
        "identifier" => {
            let name = ctx.text(expr).to_string();
            ctx.push_ref(RefKind::Call, &name, None, None, Some(0), line_of(d), Some("decorator".into()));
        }
        _ => walk(ctx, st, expr),
    }
}

fn decorator_text(ctx: &Ctx, d: Node) -> String {
    collapse_ws(ctx.text(d).trim_start_matches('@'))
}

fn dec_name(d: &str) -> &str {
    d.split('(').next().unwrap_or(d).trim()
}

fn dec_first_string(d: &str) -> Option<String> {
    let open = d.find('(')?;
    let rest = &d[open + 1..];
    let q = rest.find(['"', '\'', '`'])?;
    // Only when the string is the first argument (or the first entry of an array argument,
    // or the `path` of an options object: `@Get(['a', 'b'])`, `@Controller({ path: 'x' })`).
    let before = rest[..q].trim();
    let ok = before.is_empty() || before == "[" || (before.starts_with('{') && before.trim_end_matches(':').trim().ends_with("path"));
    if !ok {
        return None;
    }
    let quote = rest.as_bytes()[q] as char;
    let after = &rest[q + 1..];
    let end = after.find(quote)?;
    Some(after[..end].to_string())
}

fn visit_top(ctx: &mut Ctx, st: &mut State, node: Node, exported: bool, is_default: bool, decorators: Vec<String>) {
    match node.kind() {
        "import_statement" => import_stmt(ctx, node),
        "export_statement" => {
            let mut decs = decorators;
            let mut default = false;
            for ch in children(node) {
                if ch.kind() == "decorator" {
                    // Walked (refs + literals) by `visit_class`, inside the class scope.
                    decs.push(decorator_text(ctx, ch));
                }
                // `export = helper;` is the TS form of `module.exports = helper`.
                if ch.kind() == "default" || ch.kind() == "=" {
                    default = true;
                }
            }
            if let Some(decl) = node.child_by_field_name("declaration") {
                visit_top(ctx, st, decl, true, default, decs);
            } else if let Some(src) = node.child_by_field_name("source") {
                // export { a, b as c } from './x'  /  export * from './x'  /  export * as ns from './x'
                let module = string_value(src, ctx.src);
                let mut any = false;
                for ch in named_children(node) {
                    if ch.kind() == "namespace_export" {
                        let local = named_children(ch).into_iter().find(|c| c.kind() == "identifier").map(|c| ctx.text(c).to_string());
                        ctx.facts.imports.push(Import {
                            module: module.clone(),
                            imported: None,
                            local,
                            kind: ImportKind::Namespace,
                            line: line_of(node),
                        });
                        any = true;
                    }
                    if ch.kind() == "export_clause" {
                        for spec in named_children(ch) {
                            if spec.kind() == "export_specifier" {
                                let name = spec.child_by_field_name("name").map(|n| ctx.text(n).to_string());
                                let alias = spec.child_by_field_name("alias").map(|n| ctx.text(n).to_string());
                                ctx.facts.imports.push(Import {
                                    module: module.clone(),
                                    imported: name.clone(),
                                    local: alias.or(name),
                                    kind: ImportKind::Named,
                                    line: line_of(node),
                                });
                                any = true;
                            }
                        }
                    }
                }
                if !any {
                    ctx.facts.imports.push(Import {
                        module,
                        imported: None,
                        local: None,
                        kind: ImportKind::Namespace,
                        line: line_of(node),
                    });
                }
            } else {
                // export default <expr> / export { a, b }
                for ch in named_children(node) {
                    match ch.kind() {
                        "identifier" if default => {
                            let n = ctx.text(ch).to_string();
                            defer_export(st, &n, true);
                        }
                        "export_clause" => {
                            for spec in named_children(ch) {
                                if let Some(n) = spec.child_by_field_name("name") {
                                    let name = ctx.text(n).to_string();
                                    defer_export(st, &name, false);
                                    // `export { a as b }`: importers ask for `b`.
                                    if let Some(alias) = spec.child_by_field_name("alias") {
                                        let alias = ctx.text(alias).to_string();
                                        defer_alias(st, &name, &alias);
                                    }
                                }
                            }
                        }
                        "class" => visit_class(ctx, st, ch, true, true, Vec::new(), None),
                        // `export default { methods: … }` / `export default defineComponent({ … })`.
                        "object" | "call_expression" if default && vue_options(ctx, ch).is_some() => {
                            let obj = vue_options(ctx, ch).unwrap();
                            vue_component(ctx, st, obj, None, node, true, true);
                        }
                        "arrow_function" | "function_expression" | "function" => {
                            visit_function_like(ctx, st, "default", ch, ch, true, true, Vec::new());
                        }
                        // `export default withRouter(Page)` / `connect(…)(Page)`: the wrapped component.
                        "call_expression" if default => {
                            let mut wrapped = Vec::new();
                            wrapped_components(ctx, ch, &mut wrapped);
                            for w in wrapped {
                                defer_export(st, &w, true);
                            }
                            walk(ctx, st, ch);
                        }
                        _ => walk(ctx, st, ch),
                    }
                }
            }
        }
        "class_declaration" | "abstract_class_declaration" | "class" => visit_class(ctx, st, node, exported, is_default, decorators, None),
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            let name = node.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_else(|| "default".into());
            // `function f(a: string): void;` overload signatures belong to the implementation.
            if node.kind() == "function_signature" && has_implementation(ctx, node, &name) {
                return;
            }
            visit_function_like(ctx, st, &name, node, node, exported, is_default, decorators);
        }
        "lexical_declaration" | "variable_declaration" => var_decl_top(ctx, st, node, exported),
        "interface_declaration" => visit_interface(ctx, node, exported),
        "type_alias_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                let mut s = NewSymbol::new(SymbolKind::TypeAlias, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
                s.signature = truncate(&collapse_ws(ctx.text(node)), 200);
                s.is_exported = exported;
                s.visibility = vis_for_export(st, exported);
                s.docstring = doc_comment(node, ctx.src);
                s.body_hash = token_hash(node, ctx.src);
                let id = ctx.add_symbol(s);
                ctx.push_scope(Scope {
                    symbol_id: id,
                    kind: Some(SymbolKind::TypeAlias),
                    qname: ctx.child_qname(&name),
                    tparams: ts_type_params(ctx, node),
                    ..Default::default()
                });
                type_uses(ctx, node.child_by_field_name("value"));
                // `type Props = { user: User }` / `Base & { extra: X }`: members like an interface's.
                if let Some(v) = node.child_by_field_name("value") {
                    let objects: Vec<Node> = match v.kind() {
                        "object_type" => vec![v],
                        "intersection_type" => named_children(v).into_iter().filter(|c| c.kind() == "object_type").collect(),
                        _ => Vec::new(),
                    };
                    for o in objects {
                        type_members(ctx, o, false);
                    }
                }
                ctx.pop_scope();
            }
        }
        "enum_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                let mut s = NewSymbol::new(SymbolKind::Enum, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
                s.signature = format!("enum {name}");
                s.is_exported = exported;
                s.visibility = vis_for_export(st, exported);
                s.body_hash = token_hash(node, ctx.src);
                let id = ctx.add_symbol(s);
                ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Enum), qname: ctx.child_qname(&name), ..Default::default() });
                if let Some(body) = node.child_by_field_name("body") {
                    for m in named_children(body) {
                        let mn_node = if m.kind() == "enum_assignment" { m.child_by_field_name("name") } else { Some(m) };
                        if let Some(mn) = mn_node {
                            if matches!(mn.kind(), "property_identifier" | "identifier" | "string") {
                                let mname = string_like(mn, ctx.src).unwrap_or_else(|| ctx.text(mn).to_string());
                                let mut cs = NewSymbol::new(SymbolKind::Const, &mname, ctx.child_qname(&mname), line_of(m), end_line_of(m));
                                cs.signature = format!("{name}.{mname}");
                                cs.value_text = Some(ctx.text(m).to_string());
                                cs.type_ref = Some(name.clone());
                                cs.body_hash = token_hash(m, ctx.src);
                                ctx.add_symbol(cs);
                            }
                        }
                    }
                }
                ctx.pop_scope();
            }
        }
        "internal_module" | "module" => visit_namespace(ctx, st, node),
        "expression_statement" => {
            match node.named_child(0) {
                // `namespace A.B { … }` parses as an expression statement.
                Some(m) if m.kind() == "internal_module" => visit_namespace(ctx, st, m),
                // module.exports = … / exports.x = …
                Some(a) if a.kind() == "assignment_expression" && commonjs_export(ctx, st, a) => {}
                // Foo.prototype.m = function … / Foo.m = function …
                Some(a) if a.kind() == "assignment_expression" && prototype_member(ctx, st, a) => {}
                _ => walk(ctx, st, node),
            }
        }
        "ambient_declaration" | "statement_block" => {
            // `declare global { … }`, `declare module "x" { … }`, `declare function f(): void;`
            for ch in named_children(node) {
                visit_top(ctx, st, ch, exported, false, Vec::new());
            }
        }
        "import_alias" => {}
        "comment" => {}
        _ => walk(ctx, st, node),
    }
}

fn vis_for_export(st: &State, exported: bool) -> String {
    if exported || !st.has_exports {
        "public".into()
    } else {
        "private".into()
    }
}

/// `namespace A.B { … }` / `declare module "x" { … }`: members get `A.B.` qualified names.
fn visit_namespace(ctx: &mut Ctx, st: &mut State, node: Node) {
    let name = node.child_by_field_name("name").filter(|n| n.kind() != "string").map(|n| ctx.text(n).to_string()).unwrap_or_default();
    let Some(body) = node.child_by_field_name("body") else { return };
    let pushed = !name.is_empty();
    if pushed {
        let enclosing = ctx.enclosing();
        ctx.push_scope(Scope { symbol_id: enclosing, qname: ctx.child_qname(&name), ..Default::default() });
    }
    for ch in named_children(body) {
        visit_top(ctx, st, ch, false, false, Vec::new());
    }
    if pushed {
        ctx.pop_scope();
    }
}

/// `module.exports = …`, `module.exports.x = …`, `exports.x = …`.
fn commonjs_export(ctx: &mut Ctx, st: &mut State, assign: Node) -> bool {
    let (Some(left), Some(right)) = (assign.child_by_field_name("left"), assign.child_by_field_name("right")) else { return false };
    let lt = ctx.text(left).to_string();
    let target = if lt == "module.exports" {
        None
    } else if let Some(n) = lt.strip_prefix("module.exports.").or_else(|| lt.strip_prefix("exports.")) {
        Some(n.to_string())
    } else {
        return false;
    };
    st.has_exports = true;
    match (target, right.kind()) {
        (Some(name), _) if is_function_value(right) => visit_function_like(ctx, st, &name, right, assign, true, false, Vec::new()),
        (Some(name), "class") => visit_class(ctx, st, right, true, false, Vec::new(), Some(&name)),
        (Some(name), "identifier") => {
            let local = ctx.text(right).to_string();
            defer_export(st, &local, false);
            if local != name {
                defer_alias(st, &local, &name);
            }
        }
        (None, "identifier") => {
            let local = ctx.text(right).to_string();
            defer_export(st, &local, true);
        }
        (None, _) if is_function_value(right) => visit_function_like(ctx, st, "default", right, assign, true, true, Vec::new()),
        (None, "class") => visit_class(ctx, st, right, true, true, Vec::new(), None),
        (None, "object") => {
            for p in named_children(right) {
                match p.kind() {
                    "shorthand_property_identifier" => {
                        let n = ctx.text(p).to_string();
                        defer_export(st, &n, false);
                    }
                    "pair" => {
                        let key = p.child_by_field_name("key").map(|k| ctx.text(k).trim_matches(['"', '\'']).to_string());
                        let value = p.child_by_field_name("value");
                        match (key, value) {
                            (Some(k), Some(v)) if is_function_value(v) => visit_function_like(ctx, st, &k, v, p, true, false, Vec::new()),
                            (Some(k), Some(v)) if v.kind() == "identifier" => {
                                let local = ctx.text(v).to_string();
                                defer_export(st, &local, false);
                                if local != k {
                                    defer_alias(st, &local, &k);
                                }
                            }
                            (_, Some(v)) => walk(ctx, st, v),
                            _ => {}
                        }
                    }
                    "method_definition" => visit_method(ctx, st, p, &ClassInfo::default(), Vec::new()),
                    _ => walk(ctx, st, p),
                }
            }
        }
        _ => {
            walk(ctx, st, right);
        }
    }
    true
}

/// Pre-class JavaScript: `Foo.prototype.run = function () {…}` is a method of `Foo` (and
/// `Foo.create = function` a static one) when `Foo` is a function / class of this module.
fn prototype_member(ctx: &mut Ctx, st: &mut State, assign: Node) -> bool {
    let (Some(left), Some(right)) = (assign.child_by_field_name("left"), assign.child_by_field_name("right")) else { return false };
    if left.kind() != "member_expression" || !is_function_value(right) {
        return false;
    }
    let Some(member) = left.child_by_field_name("property").map(|p| ctx.text(p).to_string()) else { return false };
    let Some(obj) = left.child_by_field_name("object") else { return false };
    let (owner, is_static) = match obj.kind() {
        "identifier" => (ctx.text(obj).to_string(), true),
        "member_expression" if obj.child_by_field_name("property").map(|p| ctx.text(p)) == Some("prototype") => {
            match obj.child_by_field_name("object").filter(|o| o.kind() == "identifier") {
                Some(o) => (ctx.text(o).to_string(), false),
                None => return false,
            }
        }
        _ => return false,
    };
    let module_id = ctx.module_id();
    let Some(owner_sym) = ctx.facts.symbols.iter_mut().find(|s| {
        s.name == owner && s.parent_id.as_deref() == Some(module_id.as_str()) && matches!(s.kind, SymbolKind::Function | SymbolKind::Class)
    }) else {
        return false;
    };
    // A constructor function with prototype methods is a class (`this.m()` resolves on it).
    if !is_static {
        owner_sym.kind = SymbolKind::Class;
    }
    let (owner_id, owner_qname, owner_exported) = (owner_sym.id.clone(), owner_sym.qualified_name.clone(), owner_sym.is_exported);
    ctx.push_scope(Scope { symbol_id: owner_id, kind: Some(SymbolKind::Class), qname: owner_qname, ..Default::default() });
    let before = ctx.facts.symbols.len();
    visit_function_like(ctx, st, &member, right, assign, false, false, Vec::new());
    if let Some(m) = ctx.facts.symbols.get_mut(before) {
        m.kind = SymbolKind::Method;
        m.is_static = is_static;
        m.visibility = "public".into();
        m.is_exported = owner_exported;
    }
    ctx.pop_scope();
    true
}

/// `lazy(() => import('./X'))`, `dynamic(() => import('./X').then((m) => m.Named))`,
/// `defineAsyncComponent(() => import('./X.vue'))` → (module, imported name).
fn lazy_import(ctx: &Ctx, v: Node) -> Option<(String, String)> {
    if v.kind() != "call_expression" {
        return None;
    }
    let f = v.child_by_field_name("function")?;
    if !matches!(last_segment(ctx.text(f)), "lazy" | "dynamic" | "loadable" | "defineAsyncComponent" | "lazyLoad") {
        return None;
    }
    let loader = v.child_by_field_name("arguments")?.named_child(0).filter(|a| is_function_value(*a))?;
    let body = loader.child_by_field_name("body")?;
    let (call, picked) = match body.kind() {
        "call_expression" if body.child_by_field_name("function").map(|f| f.kind() == "import").unwrap_or(false) => (body, None),
        // `import('./x').then((m) => m.Named)`
        "call_expression" => {
            let then = body.child_by_field_name("function").filter(|f| f.kind() == "member_expression")?;
            if then.child_by_field_name("property").map(|p| ctx.text(p)) != Some("then") {
                return None;
            }
            let inner = then.child_by_field_name("object").filter(|o| o.kind() == "call_expression")?;
            let pick = body.child_by_field_name("arguments")?.named_child(0)?.child_by_field_name("body")?;
            let named = match pick.kind() {
                "member_expression" => pick.child_by_field_name("property").map(|p| ctx.text(p).to_string()),
                _ => None,
            };
            (inner, named)
        }
        _ => return None,
    };
    if call.child_by_field_name("function").map(|f| f.kind()) != Some("import") {
        return None;
    }
    let (module, _) = first_string_arg(call.child_by_field_name("arguments")?, ctx.src)?;
    Some((module, picked.unwrap_or_else(|| "default".into())))
}

/// The options object of a Vue component: `defineComponent({ … })` / `Vue.extend({ … })`, or a
/// plain object with component options.
fn vue_options<'t>(ctx: &Ctx, v: Node<'t>) -> Option<Node<'t>> {
    let obj = match v.kind() {
        "call_expression" => {
            let f = v.child_by_field_name("function")?;
            if !matches!(ctx.text(f), "defineComponent" | "Vue.extend" | "defineNuxtComponent" | "Vue.component") {
                return None;
            }
            v.child_by_field_name("arguments")?.named_children(&mut v.walk()).find(|a| a.kind() == "object")?
        }
        "object" => v,
        _ => return None,
    };
    const OPTION_KEYS: &[&str] = &["methods", "computed", "data", "setup", "props", "watch", "template", "render", "mounted", "created"];
    let is_component = named_children(obj).iter().any(|p| {
        let key = match p.kind() {
            "pair" => p.child_by_field_name("key").map(|k| ctx.text(k).to_string()),
            "method_definition" => Some(member_name(ctx, *p)),
            _ => None,
        };
        key.map(|k| OPTION_KEYS.contains(&k.as_str())).unwrap_or(false)
    });
    (is_component || v.kind() == "call_expression").then_some(obj)
}

/// A Vue options-API component as a class: `methods` / `computed` / `watch` members and the
/// lifecycle hooks are its methods, so `this.save()` resolves inside it (and from its template).
#[allow(clippy::too_many_arguments)]
fn vue_component(ctx: &mut Ctx, st: &mut State, obj: Node, name_hint: Option<&str>, range: Node, exported: bool, is_default: bool) {
    let declared = object_string_prop(ctx, obj, "name").filter(|n| is_upper_ident(n));
    let name = name_hint.map(String::from).or(declared).unwrap_or_else(|| {
        let stem = file_stem(ctx.path);
        let mut c = stem.chars();
        c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_else(|| "default".into())
    });
    let mut s = NewSymbol::new(SymbolKind::Class, &name, ctx.child_qname(&name), line_of(range), end_line_of(range));
    s.signature = format!("{name} (vue component)");
    s.is_exported = exported;
    s.visibility = vis_for_export(st, exported);
    s.docstring = doc_comment(range, ctx.src);
    s.body_hash = token_hash(obj, ctx.src);
    s.attrs = vec!["vue component".into()];
    if is_default {
        s.attrs.push("export default".into());
    }
    let id = ctx.add_symbol(s);
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Class), qname, ..Default::default() });
    let member = |ctx: &mut Ctx, st: &mut State, p: Node| {
        let before = ctx.facts.symbols.len();
        match p.kind() {
            "method_definition" => visit_method(ctx, st, p, &ClassInfo::default(), Vec::new()),
            "pair" => {
                let key = p.child_by_field_name("key").map(|k| string_like(k, ctx.src).unwrap_or_else(|| ctx.text(k).to_string()));
                match (key, p.child_by_field_name("value")) {
                    (Some(k), Some(v)) if is_function_value(v) => {
                        visit_function_like(ctx, st, &k, v, p, false, false, Vec::new());
                        if let Some(m) = ctx.facts.symbols.get_mut(before) {
                            m.kind = SymbolKind::Method;
                            m.visibility = "public".into();
                            m.is_exported = true;
                        }
                    }
                    _ => walk(ctx, st, p),
                }
            }
            _ => walk(ctx, st, p),
        }
        if let Some(m) = ctx.facts.symbols.get_mut(before).filter(|m| crate::extract::sfc::is_vue_lifecycle(&m.name)) {
            m.is_entrypoint = true;
            m.entry_kind = Some("vue lifecycle".into());
        }
    };
    for p in named_children(obj) {
        let key = (p.kind() == "pair").then(|| p.child_by_field_name("key").map(|k| ctx.text(k).to_string())).flatten();
        let value = p.child_by_field_name("value");
        match (key.as_deref(), value) {
            (Some("methods" | "computed" | "watch"), Some(v)) if v.kind() == "object" => {
                for m in named_children(v) {
                    member(ctx, st, m);
                }
            }
            _ => member(ctx, st, p),
        }
    }
    ctx.pop_scope();
}

/// Top-level `name` symbol also exported as `alias` (`export { a as b }`).
fn export_alias(ctx: &mut Ctx, name: &str, alias: &str) {
    let module_id = ctx.module_id();
    for s in ctx.facts.symbols.iter_mut() {
        if s.name == name && s.parent_id.as_deref() == Some(module_id.as_str()) {
            let m = format!("export-as:{alias}");
            if !s.modifiers.contains(&m) {
                s.modifiers.push(m);
            }
        }
    }
}

/// Upper-case identifiers passed to (possibly curried) wrapper calls: `withRouter(Page)`.
fn wrapped_components(ctx: &Ctx, call: Node, out: &mut Vec<String>) {
    if let Some(f) = call.child_by_field_name("function") {
        if f.kind() == "call_expression" {
            wrapped_components(ctx, f, out);
        }
    }
    if let Some(args) = call.child_by_field_name("arguments") {
        for a in named_children(args) {
            match a.kind() {
                "identifier" if is_upper_ident(ctx.text(a)) => out.push(ctx.text(a).to_string()),
                "call_expression" => wrapped_components(ctx, a, out),
                _ => {}
            }
        }
    }
}

/// A `function_signature` followed by a declaration of the same function (overloads).
fn has_implementation(ctx: &Ctx, sig: Node, name: &str) -> bool {
    let Some(parent) = sig.parent() else { return false };
    let container = if parent.kind() == "export_statement" { parent.parent().unwrap_or(parent) } else { parent };
    named_children(container).into_iter().any(|c| {
        let decl = if c.kind() == "export_statement" { c.child_by_field_name("declaration").unwrap_or(c) } else { c };
        decl.kind() == "function_declaration" && decl.child_by_field_name("name").map(|n| ctx.text(n) == name).unwrap_or(false)
    })
}

fn mark_exported(ctx: &mut Ctx, name: &str, default: bool) {
    let module_id = ctx.module_id();
    for s in ctx.facts.symbols.iter_mut() {
        if s.name == name && s.parent_id.as_deref() == Some(module_id.as_str()) {
            s.is_exported = true;
            s.visibility = "public".into();
            if default && !s.attrs.iter().any(|a| a == "export default") {
                s.attrs.push("export default".into());
            }
        }
    }
}

fn import_stmt(ctx: &mut Ctx, node: Node) {
    // `import fs = require('./fsx');`
    if let Some(req) = named_children(node).into_iter().find(|c| c.kind() == "import_require_clause") {
        let local = named_children(req).into_iter().find(|c| c.kind() == "identifier").map(|c| ctx.text(c).to_string());
        if let Some(src) = req.child_by_field_name("source") {
            let module = string_value(src, ctx.src);
            ctx.facts.imports.push(Import { module, imported: None, local, kind: ImportKind::Namespace, line: line_of(node) });
        }
        return;
    }
    let Some(src) = node.child_by_field_name("source") else { return };
    let module = string_value(src, ctx.src);
    let line = line_of(node);
    let clause = named_children(node).into_iter().find(|c| c.kind() == "import_clause");
    let Some(clause) = clause else {
        ctx.facts.imports.push(Import { module, imported: None, local: None, kind: ImportKind::SideEffect, line });
        return;
    };
    for ch in named_children(clause) {
        match ch.kind() {
            "identifier" => ctx.facts.imports.push(Import {
                module: module.clone(),
                imported: Some("default".into()),
                local: Some(ctx.text(ch).to_string()),
                kind: ImportKind::Default,
                line,
            }),
            "namespace_import" => {
                let local = named_children(ch).into_iter().find(|c| c.kind() == "identifier").map(|c| ctx.text(c).to_string());
                ctx.facts.imports.push(Import { module: module.clone(), imported: None, local, kind: ImportKind::Namespace, line });
            }
            "named_imports" => {
                for spec in named_children(ch) {
                    if spec.kind() != "import_specifier" {
                        continue;
                    }
                    let name = spec.child_by_field_name("name").map(|n| ctx.text(n).to_string());
                    let alias = spec.child_by_field_name("alias").map(|n| ctx.text(n).to_string());
                    ctx.facts.imports.push(Import {
                        module: module.clone(),
                        imported: name.clone(),
                        local: alias.or(name),
                        kind: ImportKind::Named,
                        line,
                    });
                }
            }
            _ => {}
        }
    }
}

fn type_annotation_text(ctx: &Ctx, node: Option<Node>) -> Option<String> {
    node.map(|t| ctx.text(t).trim().trim_start_matches(':').trim().to_string()).filter(|t| !t.is_empty())
}

fn infer_value_type(ctx: &Ctx, value: Node) -> Option<String> {
    infer_ts_type(ctx, value, false)
}

/// Static type of an expression from syntax; calls and member accesses become deferred
/// markers resolved later against the declared return / property types.
fn infer_ts_type(ctx: &Ctx, value: Node, awaited: bool) -> Option<String> {
    match value.kind() {
        "new_expression" => value.child_by_field_name("constructor").map(|c| ctx.text(c).to_string()),
        "await_expression" => value.named_child(0).and_then(|v| infer_ts_type(ctx, v, true)),
        "as_expression" | "satisfies_expression" => value.named_child(1).map(|t| ctx.text(t).to_string()),
        // `<User>x`
        "type_assertion" => value
            .named_child(0)
            .filter(|t| t.kind() == "type_arguments")
            .and_then(|t| t.named_child(0))
            .map(|t| ctx.text(t).to_string()),
        // `users[0]` / `byId[key]` (Record<K, V>).
        "subscript_expression" => {
            let owner = infer_ts_type(ctx, value.child_by_field_name("object")?, false)?;
            if is_array_type(&owner) {
                element_type(&owner)
            } else if matches!(clean_type(&owner).as_str(), "Record" | "Array" | "ReadonlyArray") {
                generic_args(&owner).last().cloned()
            } else {
                None
            }
        }
        "parenthesized_expression" | "non_null_expression" => value.named_child(0).and_then(|v| infer_ts_type(ctx, v, awaited)),
        "this" => ctx.class_name(),
        "identifier" => ctx.var_type(ctx.text(value)),
        "call_expression" => {
            let f = value.child_by_field_name("function")?;
            let type_arg = value.child_by_field_name("type_arguments").and_then(|t| t.named_child(0)).map(|t| ctx.text(t).to_string());
            let first_class_arg = || {
                value
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                    .filter(|a| a.kind() == "identifier" && is_upper_ident(ctx.text(*a)))
                    .map(|a| ctx.text(a).to_string())
            };
            let first_arg = value.child_by_field_name("arguments").and_then(|a| a.named_child(0));
            match last_segment(ctx.text(f)) {
                // `useRef<Editor>(null)`: `.current` is an Editor.
                "useRef" => return type_arg.map(|t| format!("MutableRefObject<{t}>")),
                // `createContext<User>(…)`: a context is typed by its value, which is what
                // `useContext(ctx)` returns (also through an imported context, via `@fn`).
                "createContext" => return type_arg.or_else(|| first_arg.and_then(|a| infer_ts_type(ctx, a, false))),
                "useContext" | "inject" if first_arg.map(|a| a.kind() == "identifier").unwrap_or(false) => {
                    let ctx_name = ctx.text(first_arg.unwrap());
                    if ctx.text(f) == "useContext" || !is_upper_ident(ctx_name) {
                        return ctx.var_type(ctx_name).or_else(|| Some(member_type_marker(Some("@fn"), ctx_name, false)));
                    }
                }
                // `useMemo(() => new Cache())` / Vue `computed(() => …)`: the factory's value.
                "useMemo" | "computed" => {
                    let body = first_arg.filter(|a| is_function_value(*a)).and_then(|a| a.child_by_field_name("body"));
                    return body.filter(|b| b.kind() != "statement_block").and_then(|b| infer_ts_type(ctx, b, false));
                }
                // Playwright `base.extend<MyFixtures>({ … })`: the fixtures a test receives.
                "extend" if type_arg.is_some() => return type_arg,
                _ => {}
            }
            if f.kind() == "member_expression" {
                let method = f.child_by_field_name("property").map(|p| ctx.text(p)).unwrap_or("");
                let recv = f.child_by_field_name("object").map(|o| ctx.text(o)).unwrap_or("");
                // Phaser `this.physics.add.group({ classType: Enemy })`: a group of Enemy.
                if matches!(method, "group" | "staticGroup") {
                    let class = first_arg.or_else(|| value.child_by_field_name("arguments").and_then(|a| a.named_child(1)));
                    if let Some(c) = class.and_then(|o| object_prop(ctx, o, "classType")).filter(|c| c.kind() == "identifier") {
                        return Some(format!("Group<{}>", ctx.text(c)));
                    }
                }
                // `this.scene.get('UI')`: the scene registered under that key.
                if method == "get" && (recv == "scene" || recv.ends_with(".scene")) {
                    if let Some(key) = first_arg.and_then(|a| string_like(a, ctx.src)) {
                        return Some(member_type_marker(Some("@scene"), &key, false));
                    }
                }
            }
            // DI containers: `moduleRef.get(UsersService)`, `container.resolve<Repo>()`.
            if f.kind() == "member_expression" {
                let method = f.child_by_field_name("property").map(|p| ctx.text(p)).unwrap_or("");
                let recv = f.child_by_field_name("object").map(|o| last_segment(ctx.text(o)).to_ascii_lowercase()).unwrap_or_default();
                let container = recv.contains("container")
                    || recv.contains("injector")
                    || recv.contains("moduleref")
                    || recv.ends_with("module")
                    || recv == "app"
                    || recv == "testbed";
                if container && matches!(method, "get" | "resolve" | "getService" | "inject" | "select") {
                    if let Some(t) = type_arg.clone().or_else(first_class_arg) {
                        return Some(t);
                    }
                }
            }
            match f.kind() {
                // Angular `inject(DataService)`: the injected class.
                "identifier" if ctx.text(f) == "inject" => value
                    .child_by_field_name("arguments")
                    .and_then(|a| a.named_child(0))
                    .filter(|a| a.kind() == "identifier" && is_upper_ident(ctx.text(*a)))
                    .map(|a| ctx.text(a).to_string()),
                // `Router()` / `express.Router()` (below): a router that declares routes.
                "identifier" if ctx.text(f) == "Router" => Some("Router".into()),
                // `getSvc()`: a function's declared return type.
                "identifier" => Some(member_type_marker(Some("@fn"), ctx.text(f), awaited)),
                "member_expression" => {
                    let name = f.child_by_field_name("property").map(|p| ctx.text(p).to_string())?;
                    if name == "Router" {
                        return Some("Router".into());
                    }
                    let owner = infer_ts_type(ctx, f.child_by_field_name("object")?, false)?;
                    if is_array_type(&owner) || clean_type(&owner) == "Array" {
                        return array_method_type(&owner, &name);
                    }
                    // Phaser groups: `enemies.get(x, y)` / `getFirstDead()` is a member, `getChildren()` all.
                    if matches!(clean_type(&owner).as_str(), "Group" | "StaticGroup") {
                        let member = generic_args(&owner).into_iter().next();
                        return match name.as_str() {
                            "get" | "getFirst" | "getFirstDead" | "getFirstAlive" | "getFirstNth" | "getLast" | "create" => member,
                            "getChildren" | "getMatching" => member.map(|m| format!("{m}[]")),
                            _ => None,
                        };
                    }
                    // `Map<string, User>.get(k)` → User.
                    if matches!(clean_type(&owner).as_str(), "Map" | "ReadonlyMap" | "WeakMap") {
                        return (name == "get").then(|| generic_args(&owner).get(1).cloned()).flatten();
                    }
                    Some(member_type_marker(Some(&owner), &name, awaited))
                }
                _ => None,
            }
        }
        "member_expression" => {
            let name = value.child_by_field_name("property").map(|p| ctx.text(p).to_string())?;
            let object = value.child_by_field_name("object")?;
            if object.kind() == "this" {
                if let Some(t) = ctx.field_type(&name) {
                    return Some(t);
                }
            }
            let owner = infer_ts_type(ctx, object, false)?;
            // React refs: `ref.current`.
            if name == "current" && matches!(clean_type(&owner).as_str(), "MutableRefObject" | "RefObject") {
                return generic_args(&owner).into_iter().next();
            }
            Some(member_type_marker(Some(&owner), &name, awaited))
        }
        _ => None,
    }
}

/// Result type of an `Array.prototype` method on a known array type (None: not a project type).
fn array_method_type(array: &str, method: &str) -> Option<String> {
    match method {
        "find" | "findLast" | "at" | "pop" | "shift" => element_type(array),
        "filter" | "slice" | "sort" | "reverse" | "concat" | "toSorted" | "toReversed" | "splice" => Some(array.to_string()),
        _ => None,
    }
}

/// Element type of a collection expression's type; a deferred member type gets `!elem`.
fn ts_element_type(t: &str) -> Option<String> {
    if t.starts_with("@mt#") {
        return (!t.ends_with("!elem")).then(|| format!("{t}!elem"));
    }
    element_type(t)
}

/// What awaiting a value of type `t` gives: `Promise<User>` → `User`; deferred types get `!await`.
fn ts_awaited_type(t: &str) -> Option<String> {
    if t.starts_with("@mt#") {
        return (!t.ends_with("!await") && !t.ends_with("!elem")).then(|| format!("{t}!await"));
    }
    match clean_type(t).as_str() {
        "Promise" | "PromiseLike" => generic_args(t).into_iter().next(),
        _ => None,
    }
}

/// Array methods whose callback's first parameter is an element of the receiver.
const ELEMENT_CALLBACK_METHODS: &[&str] = &[
    "map", "forEach", "filter", "find", "findLast", "findIndex", "findLastIndex", "some", "every", "flatMap", "sort", "toSorted",
];

fn is_function_value(n: Node) -> bool {
    matches!(n.kind(), "arrow_function" | "function_expression" | "function" | "generator_function")
}

fn var_decl_top(ctx: &mut Ctx, st: &mut State, node: Node, exported: bool) {
    let is_const = children(node).iter().any(|c| c.kind() == "const");
    for d in named_children(node) {
        if d.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = d.child_by_field_name("name") else { continue };
        let value = d.child_by_field_name("value");
        let ty = type_annotation_text(ctx, d.child_by_field_name("type")).or_else(|| jsdoc(ctx, node).ty);
        // require() imports
        if let Some(v) = value {
            if let Some(module) = require_module(ctx, v) {
                let line = line_of(d);
                match name_node.kind() {
                    "identifier" => ctx.facts.imports.push(Import {
                        module,
                        imported: None,
                        local: Some(ctx.text(name_node).to_string()),
                        kind: ImportKind::Namespace,
                        line,
                    }),
                    "object_pattern" => {
                        for p in named_children(name_node) {
                            let (imported, local) = match p.kind() {
                                "shorthand_property_identifier_pattern" => {
                                    let n = ctx.text(p).to_string();
                                    (n.clone(), n)
                                }
                                "pair_pattern" => (
                                    p.child_by_field_name("key").map(|k| ctx.text(k).to_string()).unwrap_or_default(),
                                    p.child_by_field_name("value").map(|k| ctx.text(k).to_string()).unwrap_or_default(),
                                ),
                                _ => continue,
                            };
                            ctx.facts.imports.push(Import {
                                module: module.clone(),
                                imported: Some(imported),
                                local: Some(local),
                                kind: ImportKind::Named,
                                line,
                            });
                        }
                    }
                    _ => {}
                }
                continue;
            }
        }
        if name_node.kind() != "identifier" {
            // Destructuring: declare names, walk value. Exported names are module members.
            let src = destructured_type(ctx, value, ty.clone());
            record_setter(ctx, st, name_node, value);
            declare_typed_pattern(ctx, name_node, src);
            if exported {
                let mut names = Vec::new();
                collect_pattern_names(ctx, name_node, &mut names);
                let kind = if is_const { SymbolKind::Const } else { SymbolKind::Variable };
                for n in names {
                    let mut s = NewSymbol::new(kind, &n, ctx.child_qname(&n), line_of(node), end_line_of(node));
                    s.signature = format!("{} {{ {n} }}", if is_const { "const" } else { "let" });
                    s.is_exported = true;
                    s.visibility = "public".into();
                    s.body_hash = token_hash(d, ctx.src);
                    ctx.add_symbol(s);
                }
            }
            if let Some(v) = value {
                walk(ctx, st, v);
            }
            continue;
        }
        let name = ctx.text(name_node).to_string();
        if let Some(v) = value {
            // `const Settings = lazy(() => import('./Settings'))`: the imported component itself.
            if let Some((module, imported)) = lazy_import(ctx, v) {
                let kind = if imported == "default" { ImportKind::Default } else { ImportKind::Named };
                ctx.facts.imports.push(Import { module, imported: Some(imported), local: Some(name), kind, line: line_of(d) });
                continue;
            }
            if is_function_value(v) {
                // `const handler: Handler = (req) => …`: the declared type is used.
                type_uses(ctx, d.child_by_field_name("type"));
                visit_function_like(ctx, st, &name, v, node, exported, false, Vec::new());
                continue;
            }
            // `export const Comp = defineComponent({ … })`: a Vue options-API component.
            if let Some(obj) = vue_options(ctx, v) {
                vue_component(ctx, st, obj, Some(&name), node, exported, false);
                continue;
            }
            // Koa `new Router({ prefix: '/api' })`: every route of the router is under the prefix.
            if v.kind() == "new_expression" && v.child_by_field_name("constructor").map(|c| last_segment(ctx.text(c)) == "Router").unwrap_or(false) {
                let prefix = v.child_by_field_name("arguments").and_then(|a| a.named_child(0)).and_then(|o| object_string_prop(ctx, o, "prefix"));
                if let Some(prefix) = prefix.filter(|p| p.starts_with('/')) {
                    st.mounts.push(Mount { prefix, target: name.clone(), parent: String::new() });
                }
            }
            // `const Foo = class { … }` is named by its declarator.
            if v.kind() == "class" {
                visit_class(ctx, st, v, exported, false, Vec::new(), Some(&name));
                continue;
            }
            // `const Row = memo((props) => …)`: a component, not a plain variable.
            if v.kind() == "call_expression" {
                let wrapper = v.child_by_field_name("function").map(|f| last_segment(ctx.text(f)).to_string()).unwrap_or_default();
                let inner = v.child_by_field_name("arguments").and_then(|a| named_children(a).into_iter().next());
                if let (true, Some(inner)) = (COMPONENT_WRAPPERS.contains(&wrapper.as_str()), inner.filter(|i| is_function_value(*i))) {
                    visit_function_like(ctx, st, &name, inner, node, exported, false, vec![format!("{wrapper}(…)")]);
                    continue;
                }
            }
        }
        let inferred = ty.clone().or_else(|| value.and_then(|v| infer_value_type(ctx, v)));
        ctx.declare_local(&name, inferred.clone());
        let literal_value = value.map(|v| {
            matches!(v.kind(), "string" | "number" | "template_string" | "true" | "false" | "regex")
                || (v.kind() == "binary_expression" && !ctx.text(v).contains('('))
        });
        let kind = if is_const && (is_const_name(&name) || literal_value == Some(true)) { SymbolKind::Const } else { SymbolKind::Variable };
        let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
        let kw = if is_const { "const" } else { "let" };
        s.signature = match (&ty, value) {
            (Some(t), _) => format!("{kw} {name}: {t}"),
            (None, Some(v)) if kind == SymbolKind::Const => format!("{kw} {name} = {}", truncate(&collapse_ws(ctx.text(v)), 80)),
            _ => format!("{kw} {name}"),
        };
        s.type_ref = inferred;
        s.value_text = value.map(|v| ctx.text(v).to_string());
        s.is_exported = exported;
        s.visibility = vis_for_export(st, exported);
        s.docstring = doc_comment(node, ctx.src);
        s.body_hash = token_hash(d, ctx.src);
        let id = ctx.add_symbol(s);
        ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
        type_uses(ctx, d.child_by_field_name("type"));
        if let Some(v) = value {
            let stores = store_objects(ctx, v);
            if v.kind() == "object" {
                // `export const api = { list() {…}, get: () => … }`: members are methods of `api`.
                object_members(ctx, st, v);
            } else if !stores.is_empty() {
                // `export const useCart = create((set) => ({ add: (x) => set(…) }))`.
                let outer = st.store.replace(name.clone());
                for o in stores {
                    object_members(ctx, st, o);
                }
                st.store = outer;
            } else {
                walk(ctx, st, v);
            }
            // Angular `export const routes: Routes = [{ path: 'users', component: UsersComponent }]`.
            if v.kind() == "array" && ty.as_deref().map(|t| clean_type(t) == "Routes").unwrap_or(false) {
                route_objects(ctx, v, "", "angular");
            }
        }
        ctx.pop_scope();
    }
}

fn object_members(ctx: &mut Ctx, st: &mut State, obj: Node) {
    for p in named_children(obj) {
        match p.kind() {
            "method_definition" => visit_method(ctx, st, p, &ClassInfo::default(), Vec::new()),
            "pair" => {
                let key = p.child_by_field_name("key").filter(|k| k.kind() != "computed_property_name");
                match (key, p.child_by_field_name("value")) {
                    (Some(k), Some(v)) if is_function_value(v) => {
                        let name = string_like(k, ctx.src).unwrap_or_else(|| ctx.text(k).to_string());
                        let before = ctx.facts.symbols.len();
                        visit_function_like(ctx, st, &name, v, p, false, false, Vec::new());
                        // Same as `name() {}` in the literal: a public method of the object.
                        if let Some(member) = ctx.facts.symbols.get_mut(before) {
                            member.kind = SymbolKind::Method;
                            member.visibility = "public".into();
                            member.is_exported = true;
                        }
                    }
                    _ => walk(ctx, st, p),
                }
            }
            _ => walk(ctx, st, p),
        }
    }
}

fn require_module(ctx: &Ctx, v: Node) -> Option<String> {
    if v.kind() != "call_expression" {
        return None;
    }
    let f = v.child_by_field_name("function")?;
    if ctx.text(f) != "require" {
        return None;
    }
    let args = v.child_by_field_name("arguments")?;
    first_string_arg(args, ctx.src).map(|(s, _)| s)
}

fn declare_pattern(ctx: &mut Ctx, pat: Node) {
    match pat.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            let n = ctx.text(pat).to_string();
            ctx.declare_local(&n, None);
        }
        _ => {
            for ch in named_children(pat) {
                match ch.kind() {
                    "pair_pattern" => {
                        if let Some(v) = ch.child_by_field_name("value") {
                            declare_pattern(ctx, v);
                        }
                    }
                    "assignment_pattern" | "object_assignment_pattern" => {
                        if let Some(l) = ch.child_by_field_name("left") {
                            declare_pattern(ctx, l);
                        }
                    }
                    _ => declare_pattern(ctx, ch),
                }
            }
        }
    }
}

/// Parameter local: (name, declared type).
type ParamLocal = (String, Option<String>);
/// Constructor parameter property: (name, type, visibility).
type ParamProp = (String, Option<String>, String);

fn parse_params(ctx: &Ctx, node: Option<Node>) -> (ParamInfo, String, Vec<ParamLocal>, Vec<ParamProp>) {
    // returns (info, text, locals, constructor property fields)
    let Some(node) = node else { return (ParamInfo::default(), String::new(), Vec::new(), Vec::new()) };
    if node.kind() == "identifier" {
        let n = ctx.text(node).to_string();
        return (
            ParamInfo::from_list(vec![Param { name: n.clone(), ty: None, optional: false, variadic: false }]),
            n.clone(),
            vec![(n, None)],
            Vec::new(),
        );
    }
    let mut list = Vec::new();
    let mut locals = Vec::new();
    let mut props = Vec::new();
    for p in named_children(node) {
        let (pattern, ty, optional, variadic, prop_vis) = match p.kind() {
            "required_parameter" | "optional_parameter" => {
                let pat = p.child_by_field_name("pattern");
                let ty = type_annotation_text(ctx, p.child_by_field_name("type"));
                let has_default = p.child_by_field_name("value").is_some();
                let mods = children(p);
                // `public` / `protected` / `private` / `readonly` / `override` make it a field;
                // without an accessibility modifier it is public.
                let prop_vis = mods.iter().any(|c| matches!(c.kind(), "accessibility_modifier" | "readonly" | "override_modifier")).then(|| {
                    mods.iter()
                        .find(|c| c.kind() == "accessibility_modifier")
                        .map(|c| ctx.text(*c).to_string())
                        .unwrap_or_else(|| "public".into())
                });
                let variadic = pat.map(|x| x.kind() == "rest_pattern").unwrap_or(false);
                (pat, ty, p.kind() == "optional_parameter" || has_default, variadic, prop_vis)
            }
            "identifier" => (Some(p), None, false, false, None),
            "assignment_pattern" => (p.child_by_field_name("left"), None, true, false, None),
            "rest_pattern" => (Some(p), None, false, true, None),
            "object_pattern" | "array_pattern" => (Some(p), None, false, false, None),
            _ => continue,
        };
        let Some(pattern) = pattern else { continue };
        let name = ctx.text(pattern).trim_start_matches("...").to_string();
        if name == "this" {
            continue;
        }
        if pattern.kind() == "identifier" || pattern.kind() == "rest_pattern" {
            locals.push((name.clone(), ty.clone()));
            if let Some(vis) = prop_vis {
                props.push((name.clone(), ty.clone(), vis));
            }
        } else {
            // `({ user, repo }: Props)`: each name is a member of the annotated type.
            pattern_locals(ctx, pattern, ty.as_deref(), &mut locals);
        }
        list.push(Param { name: collapse_ws(&name), ty, optional, variadic });
    }
    let inner = strip_parens(ctx.text(node));
    (ParamInfo::from_list(list), collapse_ws(inner), locals, props)
}

/// Locals bound by a destructuring pattern whose value has type `ty`: `{ user }` of `Props` is
/// `Props.user` (deferred), `[first]` of `User[]` is a `User`.
fn pattern_locals(ctx: &Ctx, pat: Node, ty: Option<&str>, out: &mut Vec<(String, Option<String>)>) {
    let member = |name: &str| ty.map(|t| member_type_marker(Some(t), name, false));
    match pat.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push((ctx.text(pat).to_string(), ty.map(String::from))),
        "object_pattern" => {
            for ch in named_children(pat) {
                match ch.kind() {
                    "shorthand_property_identifier_pattern" => {
                        let n = ctx.text(ch).to_string();
                        out.push((n.clone(), member(&n)));
                    }
                    "pair_pattern" => {
                        let key = ch.child_by_field_name("key").map(|k| string_like(k, ctx.src).unwrap_or_else(|| ctx.text(k).to_string()));
                        if let Some(v) = ch.child_by_field_name("value") {
                            pattern_locals(ctx, v, key.and_then(|k| member(&k)).as_deref(), out);
                        }
                    }
                    "object_assignment_pattern" => {
                        if let Some(l) = ch.child_by_field_name("left") {
                            let t = (l.kind() == "shorthand_property_identifier_pattern").then(|| member(ctx.text(l))).flatten();
                            pattern_locals(ctx, l, t.as_deref(), out);
                        }
                    }
                    _ => pattern_locals(ctx, ch, None, out),
                }
            }
        }
        "array_pattern" => {
            let elem = ty.and_then(ts_element_type);
            for ch in named_children(pat) {
                pattern_locals(ctx, ch, elem.as_deref(), out);
            }
        }
        "assignment_pattern" => {
            if let Some(l) = pat.child_by_field_name("left") {
                pattern_locals(ctx, l, ty, out);
            }
        }
        "rest_pattern" => {
            for ch in named_children(pat) {
                pattern_locals(ctx, ch, None, out);
            }
        }
        _ => {}
    }
}

/// Type of the value a destructuring declaration unpacks; `useState<T>()` / `useState(init)`
/// is a `[T, setter]` pair.
fn destructured_type(ctx: &Ctx, value: Option<Node>, declared: Option<String>) -> Option<String> {
    if declared.is_some() {
        return declared;
    }
    let v = value?;
    if v.kind() == "call_expression" {
        let f = v.child_by_field_name("function")?;
        if matches!(last_segment(ctx.text(f)), "useState" | "useSignal") {
            let state = v
                .child_by_field_name("type_arguments")
                .and_then(|t| t.named_child(0))
                .map(|t| ctx.text(t).to_string())
                .or_else(|| v.child_by_field_name("arguments").and_then(|a| a.named_child(0)).and_then(|a| infer_value_type(ctx, a)))?;
            return Some(format!("Array<{state}>"));
        }
    }
    infer_value_type(ctx, v)
}

/// `const [user, setUser] = useState(…)` / `const [state, dispatch] = useReducer(…)`: calls
/// of the setter mutate the state.
fn record_setter(ctx: &Ctx, st: &mut State, pat: Node, value: Option<Node>) {
    let Some(f) = value.filter(|v| v.kind() == "call_expression").and_then(|v| v.child_by_field_name("function")) else { return };
    if pat.kind() != "array_pattern" || !matches!(last_segment(ctx.text(f)), "useState" | "useReducer" | "useImmer") {
        return;
    }
    let names: Vec<String> = named_children(pat).into_iter().filter(|n| n.kind() == "identifier").map(|n| ctx.text(n).to_string()).collect();
    if let [state, setter, ..] = names.as_slice() {
        st.setters.insert(setter.clone(), state.clone());
    }
}

/// Member objects of a state store: Zustand `create((set) => ({ … }))`, Pinia
/// `defineStore('id', { actions: { … } })`, Redux Toolkit `createSlice({ reducers: { … } })`.
fn store_objects<'t>(ctx: &Ctx, v: Node<'t>) -> Vec<Node<'t>> {
    if v.kind() != "call_expression" {
        return Vec::new();
    }
    let Some(f) = v.child_by_field_name("function") else { return Vec::new() };
    // `create<State>()((set) => …)`: the curried form.
    let fname = match f.kind() {
        "call_expression" => f.child_by_field_name("function").map(|g| last_segment(ctx.text(g)).to_string()).unwrap_or_default(),
        _ => last_segment(ctx.text(f)).to_string(),
    };
    let args: Vec<Node> = v.child_by_field_name("arguments").map(named_children).unwrap_or_default();
    let returned = |func: Node<'t>| -> Option<Node<'t>> {
        let body = func.child_by_field_name("body")?;
        let body = if body.kind() == "parenthesized_expression" { body.named_child(0)? } else { body };
        match body.kind() {
            "object" => Some(body),
            "statement_block" => {
                let mut rets = Vec::new();
                collect_returns(body, &mut rets);
                rets.into_iter().find(|r| r.kind() == "object")
            }
            _ => None,
        }
    };
    match fname.as_str() {
        "create" | "createStore" | "createWithEqualityFn" => {
            // Middlewares wrap the initializer: `create(persist((set) => ({ … }), opts))`.
            let mut cur = args.first().copied();
            while let Some(c) = cur.filter(|c| c.kind() == "call_expression") {
                cur = c.child_by_field_name("arguments").and_then(|a| a.named_child(0));
            }
            cur.filter(|c| is_function_value(*c)).and_then(returned).into_iter().collect()
        }
        "defineStore" => args
            .iter()
            .flat_map(|a| match a.kind() {
                "object" => ["actions", "getters"].iter().filter_map(|k| object_prop(ctx, *a, k)).filter(|o| o.kind() == "object").collect(),
                _ if is_function_value(*a) => returned(*a).into_iter().collect(),
                _ => Vec::new(),
            })
            .collect(),
        "createSlice" => args
            .first()
            .and_then(|o| object_prop(ctx, *o, "reducers"))
            .filter(|o| o.kind() == "object")
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn declare_typed_pattern(ctx: &mut Ctx, pat: Node, ty: Option<String>) {
    let mut locals = Vec::new();
    pattern_locals(ctx, pat, ty.as_deref(), &mut locals);
    for (n, t) in locals {
        ctx.declare_local(&n, t);
    }
}

fn collect_pattern_names(ctx: &Ctx, pat: Node, out: &mut Vec<String>) {
    match pat.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(ctx.text(pat).to_string()),
        _ => {
            for ch in named_children(pat) {
                if ch.kind() == "pair_pattern" {
                    if let Some(v) = ch.child_by_field_name("value") {
                        collect_pattern_names(ctx, v, out);
                    }
                } else if !matches!(ch.kind(), "property_identifier") {
                    collect_pattern_names(ctx, ch, out);
                }
            }
        }
    }
}

/// Handles function declarations and functions bound to variables / default exports.
#[allow(clippy::too_many_arguments)]
fn visit_function_like(
    ctx: &mut Ctx,
    st: &mut State,
    name: &str,
    func: Node,
    range: Node,
    exported: bool,
    is_default: bool,
    decorators: Vec<String>,
) {
    let params_node = func.child_by_field_name("parameters").or_else(|| func.child_by_field_name("parameter"));
    let (params, ptext, locals, _) = parse_params(ctx, params_node);
    let doc = jsdoc(ctx, range);
    let ret = type_annotation_text(ctx, func.child_by_field_name("return_type"));
    let ret_type = return_type_text(ctx, func.child_by_field_name("return_type")).or_else(|| doc.ret.clone());
    let is_async = children(func).iter().any(|c| c.kind() == "async");
    let tparams = ts_type_params(ctx, func);
    let mut s = NewSymbol::new(SymbolKind::Function, name, ctx.child_qname(name), line_of(range), end_line_of(range));
    s.signature = match &ret {
        Some(r) => format!("{name}({ptext}): {r}"),
        None => format!("{name}({ptext})"),
    };
    if is_async {
        s.signature = format!("async {}", s.signature);
    }
    s.is_async = is_async;
    s.is_exported = exported;
    s.visibility = vis_for_export(st, exported);
    s.type_ref = ret_type.clone();
    s.params = Some(params);
    s.docstring = doc_comment(range, ctx.src);
    s.complexity = cyclomatic(func, ctx.src, ctx.lang);
    s.body_hash = token_hash(range, ctx.src);
    s.attrs = decorators;
    if is_default {
        s.attrs.push("export default".into());
    }
    // Next.js route handlers / API routes.
    let mut route = None;
    if exported {
        if let Some(r) = nextjs_route(ctx.path) {
            if HTTP_VERBS.contains(&name.to_ascii_lowercase().as_str()) && name.chars().all(|c| c.is_ascii_uppercase()) {
                route = Some((name.to_string(), r));
            } else if is_default && ctx.path.contains("pages/api/") {
                route = Some(("ANY".to_string(), r));
            }
        }
    }
    if let Some((m, p)) = &route {
        s.is_entrypoint = true;
        s.entry_kind = Some(format!("route {m} {p}"));
    } else if is_default {
        // Next.js `app/**/page.tsx`, `layout.tsx`, `pages/**`: rendered by the framework.
        if let Some(p) = nextjs_page(ctx.path) {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("next {p}"));
        }
    }
    if is_upper_ident(name) && has_jsx(func) {
        s.modifiers.push("react-component".into());
    } else if name.len() > 3 && name.starts_with("use") && name[3..].starts_with(|c: char| c.is_ascii_uppercase()) {
        s.modifiers.push("react-hook".into());
    }
    let page_path = s.entry_kind.as_deref().and_then(|e| e.strip_prefix("next page ")).map(String::from);
    let line = line_of(range);
    let id = ctx.add_symbol(s);
    if let Some((m, p)) = route {
        ctx.push_route(&id, &m, &p, "nextjs", line);
    }
    // A Next.js page is what `page.goto('/x')` visits.
    if let Some(p) = page_path {
        ctx.push_route(&id, "PAGE", &p, "nextjs", line);
    }
    let mut scope = Scope {
        symbol_id: id.clone(),
        kind: Some(SymbolKind::Function),
        qname: ctx.child_qname(name),
        is_function: true,
        ..Default::default()
    };
    let mut locals = locals;
    fastify_param_types(st, &mut locals);
    for (n, t) in locals {
        let t = t.or_else(|| doc.params.get(&n).cloned());
        scope.locals.insert(n, subst_t(&tparams, t));
    }
    scope.tparams = tparams;
    ctx.push_scope(scope);
    type_uses(ctx, params_node);
    type_uses(ctx, func.child_by_field_name("return_type"));
    if let Some(p) = params_node {
        walk_param_defaults(ctx, st, p);
    }
    if let Some(b) = func.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    if ret_type.is_none() {
        set_inferred_return(ctx, &id, func, is_async);
    }
    ctx.pop_scope();
}

/// The function renders JSX (a React component), not counting nested functions.
fn has_jsx(func: Node) -> bool {
    let mut stack = vec![func.child_by_field_name("body")];
    while let Some(Some(n)) = stack.pop() {
        if matches!(n.kind(), "jsx_element" | "jsx_self_closing_element" | "jsx_fragment") {
            return true;
        }
        if n.id() != func.id() && matches!(n.kind(), "function_declaration" | "class_declaration" | "class") {
            continue;
        }
        stack.extend(named_children(n).into_iter().map(Some));
    }
    false
}

/// Declared return type for resolution: a type guard (`x is User`) returns a boolean and an
/// assertion function (`asserts x`) returns nothing.
fn return_type_text(ctx: &Ctx, rt: Option<Node>) -> Option<String> {
    match rt?.kind() {
        "type_predicate_annotation" => Some("boolean".into()),
        "asserts_annotation" => Some("void".into()),
        _ => type_annotation_text(ctx, rt),
    }
}

/// Unannotated function: the type its body returns (`=> new Svc()`, `return this`, a typed
/// local). Must run while the function scope (its locals) is still pushed.
fn set_inferred_return(ctx: &mut Ctx, id: &str, func: Node, is_async: bool) {
    if let Some(t) = infer_return(ctx, func, is_async) {
        if let Some(s) = ctx.symbol_mut(id) {
            s.type_ref = Some(t);
        }
    }
}

fn infer_return(ctx: &Ctx, func: Node, is_async: bool) -> Option<String> {
    if func.kind().contains("generator") || children(func).iter().any(|c| c.kind() == "*") {
        return None;
    }
    let body = func.child_by_field_name("body")?;
    let mut exprs = Vec::new();
    if body.kind() == "statement_block" {
        collect_returns(body, &mut exprs);
    } else {
        exprs.push(body);
    }
    let mut found: Option<String> = None;
    for e in exprs {
        if matches!(e.kind(), "null" | "undefined") {
            continue;
        }
        let t = ctx.subst_tparam(infer_ts_type(ctx, e, false))?;
        // Deferred markers name members relative to this file's scopes: not a declared type.
        if t.starts_with("@mt#") || found.as_ref().is_some_and(|f| *f != t) {
            return None;
        }
        found = Some(t);
    }
    found.map(|t| if is_async && clean_type(&t) != "Promise" { format!("Promise<{t}>") } else { t })
}

/// `return` expressions of a function body, not of the functions / classes nested in it.
fn collect_returns<'t>(node: Node<'t>, out: &mut Vec<Node<'t>>) {
    for ch in named_children(node) {
        match ch.kind() {
            "return_statement" => out.extend(ch.named_child(0).filter(|e| e.kind() != "comment")),
            "arrow_function"
            | "function_expression"
            | "function"
            | "function_declaration"
            | "generator_function"
            | "generator_function_declaration"
            | "class"
            | "class_declaration"
            | "method_definition" => {}
            _ => collect_returns(ch, out),
        }
    }
}

/// JSDoc types of a declaration (`@param {T} name`, `@returns {T}`, `@type {T}`): the only
/// type information plain JavaScript has.
#[derive(Default)]
struct JsDoc {
    params: HashMap<String, String>,
    ret: Option<String>,
    ty: Option<String>,
}

fn jsdoc(ctx: &Ctx, node: Node) -> JsDoc {
    let mut out = JsDoc::default();
    let target = node.parent().filter(|p| p.kind() == "export_statement").unwrap_or(node);
    let Some(c) = target.prev_sibling().filter(|c| c.kind() == "comment") else { return out };
    let text = ctx.text(c);
    if !text.starts_with("/**") || c.end_position().row + 1 < target.start_position().row {
        return out;
    }
    let mut rest = text;
    while let Some(i) = rest.find('@') {
        rest = &rest[i + 1..];
        let tag: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let Some(body) = rest[tag.len()..].trim_start().strip_prefix('{') else { continue };
        let mut depth = 1;
        let Some(end) = body.char_indices().find_map(|(j, ch)| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(j)
        }) else {
            break;
        };
        let ty = jsdoc_type(&body[..end]);
        let tail = body[end + 1..].trim_start();
        match tag.as_str() {
            "param" | "arg" | "argument" => {
                // `[name=default]` is optional; `opts.key` documents a property, not a parameter.
                let name: String =
                    tail.trim_start_matches('[').chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.')).collect();
                if let Some(t) = ty.filter(|_| !name.is_empty() && !name.contains('.')) {
                    out.params.entry(name).or_insert(t);
                }
            }
            "returns" | "return" if out.ret.is_none() => out.ret = ty,
            "type" if out.ty.is_none() => out.ty = ty,
            _ => {}
        }
    }
    out
}

/// `?User` / `!User` / `...User` / `User=` / `Array.<User>` / `import('./user').User`.
fn jsdoc_type(t: &str) -> Option<String> {
    let t = t.trim().trim_start_matches(['?', '!']).trim_start_matches("...").trim_end_matches('=').trim().replace(".<", "<");
    let t = match t.rfind(").") {
        Some(i) if t.starts_with("import(") => t[i + 2..].to_string(),
        _ => t,
    };
    (!t.is_empty() && t != "*" && t != "any").then_some(t)
}

fn walk_param_defaults(ctx: &mut Ctx, st: &mut State, params: Node) {
    for p in named_children(params) {
        if let Some(v) = p.child_by_field_name("value") {
            walk(ctx, st, v);
        }
        if p.kind() == "assignment_pattern" {
            if let Some(r) = p.child_by_field_name("right") {
                walk(ctx, st, r);
            }
        }
    }
}

/// Path segments after the Next.js `app` / `pages` directory: the directory must sit at the
/// project root or right under `src/` (so `frontend-app/src/app/…` is the app router, while an
/// unrelated `…/app/…` folder deeper in the tree is not).
fn nextjs_tail<'p>(segs: &[&'p str], dir: &str) -> Option<Vec<&'p str>> {
    let idx = segs.iter().enumerate().rev().find(|(i, s)| **s == dir && (*i == 0 || segs[*i - 1] == "src" || *i <= 2)).map(|(i, _)| i)?;
    Some(segs[idx + 1..].to_vec())
}

fn route_of(dirs: &[&str]) -> String {
    let segs: Vec<&str> = dirs.iter().copied().filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')'))).collect();
    format!("/{}", segs.join("/"))
}

/// Route path for Next.js `app/**/route.ts` and `pages/api/**` files.
fn nextjs_route(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let segs: Vec<&str> = p.split('/').collect();
    if let Some(tail) = nextjs_tail(&segs, "app") {
        let (file, dirs) = tail.split_last()?;
        if file.starts_with("route.") {
            return Some(route_of(dirs));
        }
    }
    if let Some(tail) = nextjs_tail(&segs, "pages") {
        if tail.first() == Some(&"api") {
            let joined = tail.join("/");
            let no_ext = joined.rsplit_once('.').map(|(a, _)| a).unwrap_or(&joined).to_string();
            let no_index = no_ext.trim_end_matches("/index").to_string();
            return Some(format!("/{no_index}"));
        }
    }
    None
}

/// Next.js pages / layouts rendered by the framework (`app/**/page.tsx`, `pages/**`).
fn nextjs_page(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    let segs: Vec<&str> = p.split('/').collect();
    if let Some(tail) = nextjs_tail(&segs, "app") {
        let (file, dirs) = tail.split_last()?;
        let stem = file.split('.').next().unwrap_or("");
        if matches!(stem, "page" | "layout" | "template" | "loading" | "error" | "not-found" | "default") {
            return Some(format!("{stem} {}", route_of(dirs)));
        }
    }
    if let Some(tail) = nextjs_tail(&segs, "pages") {
        if tail.first() != Some(&"api") && !tail.is_empty() {
            let joined = tail.join("/");
            let no_ext = joined.rsplit_once('.').map(|(a, _)| a).unwrap_or(&joined).to_string();
            return Some(format!("page /{}", no_ext.trim_end_matches("index").trim_end_matches('/')));
        }
    }
    None
}

fn visit_class(
    ctx: &mut Ctx,
    st: &mut State,
    node: Node,
    exported: bool,
    is_default: bool,
    decorators: Vec<String>,
    name_hint: Option<&str>,
) {
    let name = node
        .child_by_field_name("name")
        .map(|n| ctx.text(n).to_string())
        .or_else(|| name_hint.map(|s| s.to_string()))
        .unwrap_or_else(|| "default".into());
    let mut bases = Vec::new();
    let mut heritage_text = String::new();
    for ch in named_children(node) {
        if ch.kind() == "class_heritage" {
            heritage_text = collapse_ws(ctx.text(ch));
            for h in named_children(ch) {
                match h.kind() {
                    "extends_clause" | "implements_clause" => {
                        for t in named_children(h) {
                            if t.kind() != "type_arguments" {
                                bases.push(collapse_ws(ctx.text(t)));
                            }
                        }
                    }
                    _ => bases.push(collapse_ws(ctx.text(h))),
                }
            }
        }
    }
    let mut decorators = decorators;
    let mut decorator_nodes: Vec<Node> = Vec::new();
    for ch in children(node) {
        if ch.kind() == "decorator" {
            decorators.push(decorator_text(ctx, ch));
            decorator_nodes.push(ch);
        }
    }
    // `@Injectable() export class X` puts the decorators on the export statement.
    if let Some(p) = node.parent().filter(|p| p.kind() == "export_statement") {
        decorator_nodes.extend(children(p).into_iter().filter(|c| c.kind() == "decorator"));
    }
    let is_abstract = node.kind() == "abstract_class_declaration";
    let mut s = NewSymbol::new(SymbolKind::Class, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = format!(
        "{}class {name}{}",
        if is_abstract { "abstract " } else { "" },
        if heritage_text.is_empty() { String::new() } else { format!(" {heritage_text}") }
    );
    s.is_abstract = is_abstract;
    s.is_exported = exported;
    s.visibility = vis_for_export(st, exported);
    let s_bases_empty = bases.is_empty();
    s.bases = bases;
    s.docstring = doc_comment(node, ctx.src);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = false;
    let controller = decorators.iter().find(|d| dec_name(d) == "Controller");
    let route_prefix = controller.map(|d| dec_first_string(d).unwrap_or_default());
    let catch_dec = decorators.iter().find(|d| dec_name(d) == "Catch").cloned();
    let module_dec = decorators.iter().find(|d| dec_name(d) == "Module").cloned();
    s.attrs = decorators;
    if is_default {
        s.attrs.push("export default".into());
    }
    let line = line_of(node);
    let id = ctx.add_symbol(s);

    // Field types: class fields + constructor parameter properties.
    let mut fields: HashMap<String, Option<String>> = HashMap::new();
    let mut param_props: Vec<(String, Option<String>, String, Node)> = Vec::new();
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        for m in named_children(body) {
            match m.kind() {
                "public_field_definition" | "field_definition" => {
                    let n = m.child_by_field_name("name").or_else(|| m.child_by_field_name("property"));
                    if let Some(n) = n {
                        let ty = type_annotation_text(ctx, m.child_by_field_name("type"))
                            .or_else(|| jsdoc(ctx, m).ty)
                            .or_else(|| m.child_by_field_name("value").and_then(|v| infer_value_type(ctx, v)));
                        fields.insert(ctx.text(n).to_string(), ty);
                    }
                }
                // Constructor parameter properties (`constructor(private users: UsersService)`) are fields.
                "method_definition" if member_name(ctx, m) == "constructor" => {
                    let (_, _, _, props) = parse_params(ctx, m.child_by_field_name("parameters"));
                    for (n, t, vis) in props {
                        fields.insert(n.clone(), t.clone());
                        param_props.push((n, t, vis, m));
                    }
                }
                _ => {}
            }
        }
    }
    let qname = ctx.child_qname(&name);
    let tparams = ts_type_params(ctx, node);
    ctx.push_scope(Scope { symbol_id: id.clone(), kind: Some(SymbolKind::Class), qname, fields, tparams, ..Default::default() });
    for d in decorator_nodes {
        walk_decorator(ctx, st, d);
    }
    for (pname, ty, vis, ctor) in param_props {
        let mut f = NewSymbol::new(SymbolKind::Field, &pname, ctx.child_qname(&pname), line_of(ctor), line_of(ctor));
        f.signature = match &ty {
            Some(t) => format!("{pname}: {t}"),
            None => pname.clone(),
        };
        f.type_ref = ty;
        f.visibility = vis;
        f.modifiers.push("parameter-property".into());
        f.body_hash = token_hash(ctor, ctx.src);
        ctx.add_symbol(f);
    }
    if let Some(d) = &catch_dec {
        let inner = d.trim_start_matches("Catch").trim().trim_start_matches('(').trim_end_matches(')');
        let types: Vec<String> = inner.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
        let types = if types.is_empty() { vec!["*".to_string()] } else { types };
        for t in types {
            ctx.facts.global_handlers.push(GlobalHandler { handler: id.clone(), exception: t, framework: "nestjs".into(), line });
        }
    }
    if let Some(d) = &module_dec {
        nest_module(ctx, d, line);
    }
    let info = ClassInfo { route_prefix: route_prefix.clone(), is_controller: route_prefix.is_some(), has_bases: !s_bases_empty };
    if let Some(body) = body {
        let mut pending: Vec<String> = Vec::new();
        // Overload signatures (`m(a: string): void;`) are attached to their implementation.
        let implemented: Vec<String> =
            named_children(body).into_iter().filter(|m| m.kind() == "method_definition").map(|m| member_name(ctx, m)).collect();
        let mut overloads: HashMap<String, u32> = HashMap::new();
        for m in named_children(body) {
            if m.kind() == "method_signature" {
                let n = member_name(ctx, m);
                if implemented.contains(&n) {
                    *overloads.entry(n).or_default() += 1;
                }
            }
        }
        for m in named_children(body) {
            match m.kind() {
                "decorator" => {
                    pending.push(decorator_text(ctx, m));
                    walk_decorator(ctx, st, m);
                }
                "method_signature" if implemented.contains(&member_name(ctx, m)) => pending.clear(),
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    let decs = std::mem::take(&mut pending);
                    let before = ctx.facts.symbols.len();
                    visit_method(ctx, st, m, &info, decs);
                    if let Some(n) = overloads.get(&member_name(ctx, m)).filter(|_| m.kind() == "method_definition") {
                        if let Some(s) = ctx.facts.symbols.get_mut(before) {
                            s.modifiers.push(format!("overloads:{n}"));
                        }
                    }
                }
                "public_field_definition" | "field_definition" => {
                    let decs = std::mem::take(&mut pending);
                    visit_field(ctx, st, m, decs);
                }
                "class_static_block" => {
                    pending.clear();
                    walk(ctx, st, m);
                }
                _ => pending.clear(),
            }
        }
    }
    ctx.pop_scope();
}

/// Parses `@Module({ providers: [...] })` into DI registrations.
fn nest_module(ctx: &mut Ctx, decorator: &str, line: u32) {
    let Some(start) = decorator.find("providers") else { return };
    let rest = &decorator[start..];
    let Some(open) = rest.find('[') else { return };
    let mut depth = 0;
    let mut end = open;
    for (i, c) in rest[open..].char_indices() {
        match c {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => {
                depth -= 1;
                if depth == 0 {
                    end = open + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = &rest[open + 1..end];
    // Split top-level elements.
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut d = 0;
    for c in inner.chars() {
        match c {
            '{' | '[' | '(' => {
                d += 1;
                cur.push(c)
            }
            '}' | ']' | ')' => {
                d -= 1;
                cur.push(c)
            }
            ',' if d == 0 => {
                items.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        items.push(cur.trim().to_string());
    }
    for item in items {
        if item.starts_with('{') {
            let get = |key: &str| -> Option<String> {
                let i = item.find(&format!("{key}:"))?;
                let v = item[i + key.len() + 1..].trim_start();
                let v: String = v.chars().take_while(|c| *c != ',' && *c != '}').collect();
                Some(v.trim().trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string())
            };
            let Some(token) = get("provide") else { continue };
            let (implementation, kind) = if let Some(c) = get("useClass") {
                (Some(c), "class")
            } else if let Some(e) = get("useExisting") {
                (Some(e), "existing")
            } else if get("useFactory").is_some() {
                (None, "factory")
            } else {
                (None, "value")
            };
            let lifetime = if item.contains("Scope.REQUEST") {
                "request"
            } else if item.contains("Scope.TRANSIENT") {
                "transient"
            } else {
                "singleton"
            };
            let fw = format!("nestjs-{kind}");
            ctx.push_di(&token, implementation.as_deref(), lifetime, &fw, line);
        } else if !item.is_empty() && item.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            ctx.push_di(&item, Some(&item), "singleton", "nestjs", line);
        }
    }
}

/// `m()`, `'quoted'()`, `['computed']()` → `m` / `quoted` / `computed`; `[Symbol.iterator]`
/// keeps its expression.
fn member_name(ctx: &Ctx, m: Node) -> String {
    let Some(n) = m.child_by_field_name("name") else { return String::new() };
    let lit = match n.kind() {
        "computed_property_name" => n.named_child(0).filter(|e| matches!(e.kind(), "string" | "template_string")),
        "string" => Some(n),
        _ => None,
    };
    lit.and_then(|l| string_like(l, ctx.src)).unwrap_or_else(|| ctx.text(n).to_string())
}

fn visit_method(ctx: &mut Ctx, st: &mut State, m: Node, cls: &ClassInfo, decorators: Vec<String>) {
    let name = member_name(ctx, m);
    if name.is_empty() {
        return;
    }
    let toks: Vec<String> = children(m)
        .iter()
        .filter(|c| !c.is_named() || c.kind() == "accessibility_modifier" || c.kind() == "override_modifier")
        .map(|c| ctx.text(*c).to_string())
        .collect();
    let is_static = toks.iter().any(|t| t == "static");
    let is_async = toks.iter().any(|t| t == "async");
    let accessor = toks.iter().find(|t| *t == "get" || *t == "set").cloned();
    let access = toks.iter().find(|t| matches!(t.as_str(), "private" | "protected" | "public")).cloned();
    // Only `abstract` members (and interface-like signatures without body) are abstract.
    let is_abstract = m.kind() == "abstract_method_signature" || toks.iter().any(|t| t == "abstract");
    let kind = if name == "constructor" {
        SymbolKind::Constructor
    } else if accessor.is_some() {
        SymbolKind::Property
    } else {
        SymbolKind::Method
    };
    let params_node = m.child_by_field_name("parameters");
    let (params, ptext, mut locals, _) = parse_params(ctx, params_node);
    let doc = jsdoc(ctx, m);
    for (n, t) in locals.iter_mut() {
        if t.is_none() {
            *t = doc.params.get(n).cloned();
        }
    }
    let ret = type_annotation_text(ctx, m.child_by_field_name("return_type"));
    let ret_type = return_type_text(ctx, m.child_by_field_name("return_type")).or_else(|| doc.ret.clone());
    let tparams = ts_type_params(ctx, m);
    // `get total()` / `set total(v)` are one property: the second accessor reuses it.
    if kind == SymbolKind::Property {
        let q = ctx.child_qname(&name);
        if let Some(existing) =
            ctx.facts.symbols.iter().rev().find(|s| s.kind == SymbolKind::Property && s.qualified_name == q).map(|s| s.id.clone())
        {
            let mut scope = Scope { symbol_id: existing, kind: Some(kind), qname: q, is_function: true, ..Default::default() };
            for (n, t) in locals {
                scope.locals.insert(n, subst_t(&tparams, t));
            }
            ctx.push_scope(scope);
            if let Some(b) = m.child_by_field_name("body") {
                walk(ctx, st, b);
            }
            ctx.pop_scope();
            return;
        }
    }
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(m), end_line_of(m));
    let prefix = match (&accessor, is_async, is_static) {
        (Some(a), _, _) => format!("{a} "),
        (None, true, true) => "static async ".into(),
        (None, true, false) => "async ".into(),
        (None, false, true) => "static ".into(),
        _ => String::new(),
    };
    s.signature = match &ret {
        Some(r) => format!("{prefix}{name}({ptext}): {r}"),
        None => format!("{prefix}{name}({ptext})"),
    };
    s.visibility = if name.starts_with('#') { "private".into() } else { access.unwrap_or_else(|| "public".into()) };
    s.is_static = is_static;
    s.is_async = is_async;
    s.is_abstract = is_abstract;
    s.is_exported = s.visibility == "public";
    // A possible Phaser hook (`create`, `update`, `preUpdate`, …): confirmed or dropped by the
    // global pass, which follows the real inheritance chain up to a Phaser base.
    if cls.has_bases && !is_static && crate::phaser::is_hook_name(&name) {
        s.modifiers.push("phaser-hook".into());
    }
    s.type_ref = ret_type.clone();
    if kind != SymbolKind::Property {
        s.params = Some(params);
    }
    s.docstring = doc_comment(m, ctx.src);
    s.complexity = cyclomatic(m, ctx.src, ctx.lang);
    s.body_hash = token_hash(m, ctx.src);
    let mut route = None;
    for d in &decorators {
        let dn = dec_name(d);
        let verb = match dn {
            "Get" | "Post" | "Put" | "Delete" | "Patch" | "Head" | "Options" | "All" => Some(dn.to_ascii_uppercase()),
            _ => None,
        };
        if let (Some(v), true) = (verb, cls.is_controller) {
            let sub = dec_first_string(d).unwrap_or_default();
            let path = join_route(cls.route_prefix.as_deref().unwrap_or(""), &sub);
            let v = if v == "ALL" { "ANY".to_string() } else { v };
            route = Some((v, path));
        }
        if matches!(dn, "MessagePattern" | "EventPattern" | "OnEvent" | "Cron" | "Interval" | "Process" | "SubscribeMessage") {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("event handler @{dn}"));
        }
    }
    if let Some((v, p)) = &route {
        s.is_entrypoint = true;
        s.entry_kind = Some(format!("route {v} {p}"));
    }
    s.attrs = decorators;
    let line = line_of(m);
    let id = ctx.add_symbol(s);
    if let Some((v, p)) = route {
        ctx.push_route(&id, &v, &p, "nestjs", line);
    }
    let mut scope =
        Scope { symbol_id: id.clone(), kind: Some(kind), qname: ctx.child_qname(&name), is_function: true, ..Default::default() };
    for (n, t) in locals {
        scope.locals.insert(n, subst_t(&tparams, t));
    }
    scope.tparams = tparams;
    ctx.push_scope(scope);
    type_uses(ctx, params_node);
    type_uses(ctx, m.child_by_field_name("return_type"));
    if let Some(p) = params_node {
        walk_param_defaults(ctx, st, p);
    }
    if let Some(b) = m.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    if ret_type.is_none() && kind != SymbolKind::Constructor && accessor.as_deref() != Some("set") {
        set_inferred_return(ctx, &id, m, is_async);
    }
    ctx.pop_scope();
}

fn visit_field(ctx: &mut Ctx, st: &mut State, m: Node, decorators: Vec<String>) {
    let Some(n) = m.child_by_field_name("name").or_else(|| m.child_by_field_name("property")) else { return };
    let name = ctx.text(n).to_string();
    // Field decorators (`@Column() id`, `@ManyToOne(() => User) user`) are children of the field.
    let mut decorators = decorators;
    let decorator_nodes: Vec<Node> = children(m).into_iter().filter(|c| c.kind() == "decorator").collect();
    for d in &decorator_nodes {
        decorators.push(decorator_text(ctx, *d));
    }
    let toks: Vec<String> =
        children(m).iter().filter(|c| !c.is_named() || c.kind() == "accessibility_modifier").map(|c| ctx.text(*c).to_string()).collect();
    let access = toks.iter().find(|t| matches!(t.as_str(), "private" | "protected" | "public")).cloned();
    let is_static = toks.iter().any(|t| t == "static");
    let ty = type_annotation_text(ctx, m.child_by_field_name("type")).or_else(|| jsdoc(ctx, m).ty);
    let value = m.child_by_field_name("value");
    if let Some(v) = value {
        if is_function_value(v) {
            // Arrow-function class properties behave like methods (never module exports, so a
            // `GET = () => …` field is not a Next.js route handler).
            let before = ctx.facts.symbols.len();
            visit_function_like(ctx, st, &name, v, m, false, false, decorators);
            if let Some(field_fn) = ctx.facts.symbols.get_mut(before) {
                field_fn.kind = SymbolKind::Method;
                field_fn.visibility = if name.starts_with('#') { "private".into() } else { access.unwrap_or_else(|| "public".into()) };
                field_fn.is_exported = field_fn.visibility == "public";
                field_fn.is_static = is_static;
            }
            return;
        }
    }
    let mut s = NewSymbol::new(SymbolKind::Field, &name, ctx.child_qname(&name), line_of(m), end_line_of(m));
    s.signature = match &ty {
        Some(t) => format!("{name}: {t}"),
        None => name.clone(),
    };
    s.type_ref = ty.or_else(|| value.and_then(|v| infer_value_type(ctx, v)));
    s.value_text = value.map(|v| ctx.text(v).to_string());
    s.visibility = if name.starts_with('#') { "private".into() } else { access.unwrap_or_else(|| "public".into()) };
    s.is_static = is_static;
    s.attrs = decorators;
    s.body_hash = token_hash(m, ctx.src);
    let id = ctx.add_symbol(s);
    ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
    for d in decorator_nodes {
        walk_decorator(ctx, st, d);
    }
    type_uses(ctx, m.child_by_field_name("type"));
    if let Some(v) = value {
        walk(ctx, st, v);
    }
    ctx.pop_scope();
}

fn visit_interface(ctx: &mut Ctx, node: Node, exported: bool) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let mut bases = Vec::new();
    for ch in named_children(node) {
        if ch.kind() == "extends_type_clause" {
            for t in named_children(ch) {
                bases.push(collapse_ws(ctx.text(t)));
            }
        }
    }
    let mut s = NewSymbol::new(SymbolKind::Interface, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = if bases.is_empty() { format!("interface {name}") } else { format!("interface {name} extends {}", bases.join(", ")) };
    s.bases = bases;
    s.is_exported = exported;
    s.visibility = "public".into();
    s.is_abstract = true;
    s.docstring = doc_comment(node, ctx.src);
    s.body_hash = token_hash(node, ctx.src);
    let id = ctx.add_symbol(s);
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Interface),
        qname: ctx.child_qname(&name),
        tparams: ts_type_params(ctx, node),
        ..Default::default()
    });
    if let Some(body) = node.child_by_field_name("body") {
        type_members(ctx, body, true);
    }
    ctx.pop_scope();
}

/// Members of an interface body / object type (`type Props = { user: User; onSave(): void }`)
/// as properties and abstract methods of the enclosing type scope.
fn type_members(ctx: &mut Ctx, body: Node, record_uses: bool) {
    for m in named_children(body) {
        let mname = member_name(ctx, m);
        if mname.is_empty() {
            continue;
        }
        match m.kind() {
            "method_signature" => {
                let (params, ptext, _, _) = parse_params(ctx, m.child_by_field_name("parameters"));
                let ret = type_annotation_text(ctx, m.child_by_field_name("return_type"));
                let mut ms = NewSymbol::new(SymbolKind::Method, &mname, ctx.child_qname(&mname), line_of(m), end_line_of(m));
                ms.signature = match &ret {
                    Some(r) => format!("{mname}({ptext}): {r}"),
                    None => format!("{mname}({ptext})"),
                };
                ms.type_ref = ret;
                ms.params = Some(params);
                ms.is_abstract = true;
                ms.body_hash = token_hash(m, ctx.src);
                ctx.add_symbol(ms);
                if record_uses {
                    type_uses(ctx, m.child_by_field_name("parameters"));
                    type_uses(ctx, m.child_by_field_name("return_type"));
                }
            }
            "property_signature" => {
                let ty = type_annotation_text(ctx, m.child_by_field_name("type"));
                let mut ps = NewSymbol::new(SymbolKind::Property, &mname, ctx.child_qname(&mname), line_of(m), end_line_of(m));
                ps.signature = collapse_ws(ctx.text(m)).trim_end_matches([';', ',']).to_string();
                ps.type_ref = ty;
                ps.body_hash = token_hash(m, ctx.src);
                ctx.add_symbol(ps);
                if record_uses {
                    type_uses(ctx, m.child_by_field_name("type"));
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Statement / expression walker
// ---------------------------------------------------------------------------

fn walk(ctx: &mut Ctx, st: &mut State, node: Node) {
    if st.walked_handlers.contains(&node.id()) {
        return;
    }
    match node.kind() {
        "lexical_declaration" | "variable_declaration" => {
            if !ctx.in_function() && ctx.scopes.len() == 1 {
                var_decl_top(ctx, st, node, false);
                return;
            }
            for d in named_children(node) {
                if d.kind() != "variable_declarator" {
                    continue;
                }
                let value = d.child_by_field_name("value");
                if let Some(n) = d.child_by_field_name("name") {
                    // `const db = require('./db')` inside a function is still an import.
                    if let Some(module) = value.and_then(|v| require_module(ctx, v)) {
                        let local = (n.kind() == "identifier").then(|| ctx.text(n).to_string());
                        ctx.facts.imports.push(Import { module, imported: None, local, kind: ImportKind::Namespace, line: line_of(d) });
                    }
                    if n.kind() == "identifier" {
                        let name = ctx.text(n).to_string();
                        let ty = type_annotation_text(ctx, d.child_by_field_name("type"))
                            .or_else(|| jsdoc(ctx, node).ty)
                            .or_else(|| value.and_then(|v| infer_value_type(ctx, v)));
                        let ty = ctx.subst_tparam(ty);
                        ctx.declare_local(&name, ty);
                    } else {
                        let declared = type_annotation_text(ctx, d.child_by_field_name("type")).or_else(|| jsdoc(ctx, node).ty);
                        let src = destructured_type(ctx, value, declared);
                        record_setter(ctx, st, n, value);
                        declare_typed_pattern(ctx, n, src);
                    }
                }
                if let Some(v) = value {
                    walk(ctx, st, v);
                }
            }
        }
        "function_declaration" | "generator_function_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                ctx.declare_local(&name, None);
            }
            inline_function(ctx, st, node);
        }
        "arrow_function" | "function_expression" | "function" | "generator_function" => inline_function(ctx, st, node),
        "class_declaration" | "class" => {
            if ctx.in_function() {
                walk_children(ctx, st, node);
            } else {
                visit_class(ctx, st, node, false, false, Vec::new(), None);
            }
        }
        "call_expression" => call(ctx, st, node),
        "new_expression" => {
            let ctor = node.child_by_field_name("constructor").map(|c| ctx.text(c).to_string()).unwrap_or_default();
            let args = node.child_by_field_name("arguments");
            let argc = args.map(count_args).or(Some(0));
            ctx.push_ref(RefKind::New, &clean_type(&ctor), None, None, argc, line_of(node), None);
            if let Some(a) = args {
                walk(ctx, st, a);
            }
        }
        "throw_statement" => {
            let Some(expr) = node.named_child(0) else { return };
            match expr.kind() {
                "new_expression" | "call_expression" => {
                    let f = expr.child_by_field_name("constructor").or_else(|| expr.child_by_field_name("function"));
                    let ty = f.map(|f| clean_type(ctx.text(f))).unwrap_or_default();
                    let args = expr.child_by_field_name("arguments");
                    let msg = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
                    ctx.push_ref(RefKind::Throw, &ty, None, None, None, line_of(node), msg);
                    if let Some(a) = args {
                        walk(ctx, st, a);
                    }
                }
                "identifier" => {
                    let n = ctx.text(expr).to_string();
                    if let Some(t) = ctx.var_type(&n) {
                        ctx.push_ref(RefKind::Throw, &clean_type(&t), None, None, None, line_of(node), None);
                    }
                }
                _ => walk(ctx, st, expr),
            }
        }
        "try_statement" => {
            let handler = node.child_by_field_name("handler");
            // TS only allows `catch (e: unknown)` / `catch (e: any)`: every handler catches everything.
            let ty = handler.map(|_| "*".to_string()).into_iter().collect::<Vec<_>>();
            ctx.push_catch_frame(ty.clone());
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
            ctx.pop_catch_frame();
            if let Some(h) = handler {
                ctx.push_ref(RefKind::Catch, ty.first().map(|s| s.as_str()).unwrap_or("*"), None, None, None, line_of(h), None);
                if let Some(p) = h.child_by_field_name("parameter") {
                    declare_pattern(ctx, p);
                }
                if let Some(b) = h.child_by_field_name("body") {
                    walk(ctx, st, b);
                }
            }
            if let Some(f) = node.child_by_field_name("finalizer") {
                walk_children(ctx, st, f);
            }
        }
        "assignment_expression" | "augmented_assignment_expression" => {
            if let Some(l) = node.child_by_field_name("left") {
                write_target(ctx, st, l);
                if node.kind() == "augmented_assignment_expression" {
                    walk(ctx, st, l);
                }
            }
            if let Some(r) = node.child_by_field_name("right") {
                walk(ctx, st, r);
            }
        }
        "update_expression" => {
            if let Some(a) = node.child_by_field_name("argument") {
                write_target(ctx, st, a);
                walk(ctx, st, a);
            }
        }
        "member_expression" => member_expr(ctx, st, node),
        "subscript_expression" => {
            if let (Some(o), Some(i)) = (node.child_by_field_name("object"), node.child_by_field_name("index")) {
                let ot = ctx.text(o);
                if ot == "process.env" || ot == "import.meta.env" {
                    if let Some(k) = string_like(i, ctx.src) {
                        let d = binary_default(ctx, node);
                        ctx.push_config(&k, "process.env", line_of(node), d);
                    }
                }
            }
            walk_children(ctx, st, node);
        }
        "identifier" | "shorthand_property_identifier" => {
            let name = ctx.text(node).to_string();
            if ctx.is_local(&name) || BUILTIN_IDENTS.contains(&name.as_str()) {
                return;
            }
            ctx.push_ref(RefKind::Read, &name, None, None, None, line_of(node), None);
        }
        "await_expression" => {
            ctx.mark_await();
            walk_children(ctx, st, node);
        }
        "jsx_opening_element" | "jsx_self_closing_element" => {
            if let Some(n) = node.child_by_field_name("name") {
                let t = ctx.text(n).to_string();
                let simple = last_segment(&t).to_string();
                if is_upper_ident(&simple) {
                    let recv =
                        if t.contains('.') { Some(t.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_default()) } else { None };
                    let attrs = named_children(node).iter().filter(|c| c.kind() == "jsx_attribute").count() as u32;
                    ctx.push_ref(RefKind::Jsx, &simple, recv, None, Some(attrs), line_of(node), None);
                    if simple == "Route" {
                        jsx_route(ctx, node);
                    }
                }
            }
            for ch in named_children(node) {
                if Some(ch) != node.child_by_field_name("name") {
                    walk(ctx, st, ch);
                }
            }
        }
        "jsx_attribute" => {
            let kids = named_children(node);
            // `data-testid="save"`: what `getByTestId('save')` finds.
            if kids.first().map(|k| TESTID_ATTRS.contains(&ctx.text(*k))).unwrap_or(false) {
                if let Some(v) = kids.get(1).filter(|v| v.kind() == "string") {
                    let id = string_value(*v, ctx.src);
                    ctx.push_literal(&id, "testid", *v);
                    return;
                }
            }
            for ch in kids.into_iter().skip(1) {
                walk(ctx, st, ch);
            }
        }
        "jsx_text" | "jsx_closing_element" => {}
        "string" => {
            if node.parent().map(|p| matches!(p.kind(), "import_statement" | "export_statement")).unwrap_or(false) {
                return;
            }
            let v = string_value(node, ctx.src);
            ctx.push_literal(&v, "string", node);
        }
        "template_string" => {
            let v = template_text(node, ctx.src);
            let t = if has_child_kind(node, "template_substitution") { "template" } else { "string" };
            ctx.push_literal(&v, t, node);
            walk_children(ctx, st, node);
        }
        "number" => {
            let v = ctx.text(node).to_string();
            ctx.push_literal(&v, "number", node);
        }
        "regex" => {
            let v = ctx.text(node).to_string();
            ctx.push_literal(&v, "regex", node);
        }
        "for_in_statement" => {
            if let Some(l) = node.child_by_field_name("left") {
                // `for (const o of orders)` / `for await (const r of rows)`: an element of the iterable.
                let is_of = children(node).iter().any(|c| c.kind() == "of");
                let elem = node
                    .child_by_field_name("right")
                    .filter(|_| is_of && l.kind() == "identifier")
                    .and_then(|r| infer_value_type(ctx, r))
                    .and_then(|t| ts_element_type(&t));
                match elem {
                    Some(t) => {
                        let n = ctx.text(l).to_string();
                        ctx.declare_local(&n, Some(t));
                    }
                    None => declare_pattern(ctx, l),
                }
            }
            if let Some(r) = node.child_by_field_name("right") {
                walk(ctx, st, r);
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
        }
        "pair" => {
            if let Some(v) = node.child_by_field_name("value") {
                walk(ctx, st, v);
            }
            if let Some(k) = node.child_by_field_name("key") {
                if k.kind() == "computed_property_name" {
                    walk(ctx, st, k);
                }
            }
        }
        "type_annotation"
        | "type_arguments"
        | "type_parameters"
        | "type_identifier"
        | "predefined_type"
        | "property_identifier"
        | "comment"
        | "import_statement"
        | "interface_declaration"
        | "type_alias_declaration"
        | "statement_identifier" => {}
        "decorator" => walk_decorator(ctx, st, node),
        _ => walk_children(ctx, st, node),
    }
}

fn walk_children(ctx: &mut Ctx, st: &mut State, node: Node) {
    for ch in named_children(node) {
        walk(ctx, st, ch);
    }
}

fn inline_function(ctx: &mut Ctx, st: &mut State, node: Node) {
    let hint = ctx.lambda_hint.take();
    let params_node = node.child_by_field_name("parameters").or_else(|| node.child_by_field_name("parameter"));
    let (_, _, mut locals, _) = parse_params(ctx, params_node);
    if let Some(h) = hint {
        apply_param_hint(ctx, params_node, &h, &mut locals);
    }
    fastify_param_types(st, &mut locals);
    // Inline functions are part of the enclosing symbol; their parameters are locals of it.
    let mut created_scope = false;
    if !ctx.in_function() {
        // Top-level anonymous function (e.g. IIFE / callback): give it a function scope bound to the
        // enclosing symbol so locals don't leak into module scope.
        let id = ctx.enclosing();
        ctx.push_scope(Scope {
            symbol_id: id,
            qname: ctx.scopes.last().map(|s| s.qname.clone()).unwrap_or_default(),
            is_function: true,
            ..Default::default()
        });
        created_scope = true;
    }
    for (n, t) in locals {
        ctx.declare_local(&n, t);
    }
    if children(node).iter().any(|c| c.kind() == "async") {
        ctx.mark_await();
    }
    if let Some(p) = params_node {
        walk_param_defaults(ctx, st, p);
    }
    if let Some(b) = node.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    if created_scope {
        ctx.pop_scope();
    }
}

/// Types the first parameter of a callback with `hint`: `(u) => …` is a `hint`, and a
/// destructured `({ page, todoPage }) => …` takes its names from members of `hint`.
fn apply_param_hint(ctx: &Ctx, params: Option<Node>, hint: &str, locals: &mut [(String, Option<String>)]) {
    let Some(first) = params.and_then(|p| if p.kind() == "identifier" { Some(p) } else { p.named_child(0) }) else { return };
    let pattern = match first.kind() {
        "required_parameter" | "optional_parameter" if first.child_by_field_name("type").is_none() => first.child_by_field_name("pattern"),
        "identifier" | "object_pattern" | "array_pattern" => Some(first),
        _ => None,
    };
    let Some(pattern) = pattern else { return };
    let mut typed = Vec::new();
    pattern_locals(ctx, pattern, Some(hint), &mut typed);
    for (n, t) in typed {
        if let Some(slot) = locals.iter_mut().find(|(ln, lt)| *ln == n && lt.is_none()) {
            slot.1 = t;
        }
    }
}

fn binary_default(ctx: &Ctx, node: Node) -> Option<String> {
    let p = node.parent()?;
    if p.kind() == "binary_expression" && p.child_by_field_name("left") == Some(node) {
        let op = p.child_by_field_name("operator").map(|o| ctx.text(o)).unwrap_or("");
        if op == "||" || op == "??" {
            return p.child_by_field_name("right").map(|r| truncate(ctx.text(r), 120));
        }
    }
    None
}

fn member_expr(ctx: &mut Ctx, st: &mut State, node: Node) {
    let obj = node.child_by_field_name("object");
    let prop = node.child_by_field_name("property").map(|p| ctx.text(p).to_string()).unwrap_or_default();
    let Some(obj) = obj else { return };
    let ot = ctx.text(obj);
    if ot == "process.env" || ot == "import.meta.env" {
        let d = binary_default(ctx, node);
        let src = if ot == "process.env" { "process.env" } else { "import.meta.env" };
        ctx.push_config(&prop, src, line_of(node), d);
        return;
    }
    if obj.kind() == "this" {
        let cls = ctx.class_name();
        ctx.push_ref(RefKind::Read, &prop, Some("this".into()), cls, None, line_of(node), None);
        return;
    }
    // `order.total`, `Status.Active`, `Config.MAX`: a member read on a receiver.
    if !prop.is_empty() && !matches!(obj.kind(), "call_expression" | "super") {
        let (rtext, rtype) = receiver_info(ctx, obj);
        if !BUILTIN_IDENTS.contains(&rtext.as_str()) {
            ctx.push_ref(RefKind::Read, &prop, Some(rtext), rtype, None, line_of(node), None);
        }
    }
    walk(ctx, st, obj);
}

fn receiver_info(ctx: &Ctx, r: Node) -> (String, Option<String>) {
    let text = simplify_receiver(ctx.text(r));
    let ty = match r.kind() {
        "identifier" => ctx.var_type(&text),
        "this" => ctx.class_name(),
        "member_expression" => {
            let o = r.child_by_field_name("object");
            let p = r.child_by_field_name("property").map(|p| ctx.text(p).to_string());
            match (o, p) {
                (Some(o), Some(p)) if o.kind() == "this" => ctx.field_type(&p),
                _ => infer_value_type(ctx, r),
            }
        }
        "new_expression" => r.child_by_field_name("constructor").map(|c| ctx.text(c).to_string()),
        _ => infer_value_type(ctx, r),
    };
    (text, ty)
}

fn write_target(ctx: &mut Ctx, st: &mut State, left: Node) {
    let line = line_of(left);
    match left.kind() {
        "identifier" => {
            let name = ctx.text(left).to_string();
            if ctx.is_local(&name) {
                return;
            }
            if ctx.is_module_var(&name) && ctx.in_function() {
                ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
                ctx.push_mutation(&name, "global_var", line);
            } else if !ctx.is_module_var(&name) {
                ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
            }
        }
        "member_expression" => {
            let obj = left.child_by_field_name("object");
            let prop = left.child_by_field_name("property").map(|p| ctx.text(p).to_string()).unwrap_or_default();
            match obj {
                Some(o) if o.kind() == "this" => {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Write, &prop, Some("this".into()), cls.clone(), None, line, None);
                    ctx.push_mutation(&format!("{}.{}", cls.unwrap_or_default(), prop), "local_field", line);
                }
                Some(o) => {
                    let ot = ctx.text(o).to_string();
                    if ot == "module" && prop == "exports" || ot == "exports" {
                        return;
                    }
                    let (rtext, rtype) = receiver_info(ctx, o);
                    let owner = rtype.clone().map(|t| clean_type(&t)).unwrap_or_else(|| rtext.clone());
                    ctx.push_ref(RefKind::Write, &prop, Some(rtext.clone()), rtype, None, line, None);
                    let base = rtext.split('.').next().unwrap_or("").to_string();
                    let mt =
                        if !ctx.is_local(&base) && ctx.is_module_var(&base) && ctx.in_function() { "global_var" } else { "object_field" };
                    ctx.push_mutation(&format!("{owner}.{prop}"), mt, line);
                    walk(ctx, st, o);
                }
                None => {}
            }
        }
        "subscript_expression" => {
            if let Some(o) = left.child_by_field_name("object") {
                if o.kind() == "identifier" {
                    let n = ctx.text(o).to_string();
                    if !ctx.is_local(&n) && ctx.is_module_var(&n) && ctx.in_function() {
                        ctx.push_ref(RefKind::Write, &n, None, None, None, line, None);
                        ctx.push_mutation(&n, "global_var", line);
                    }
                }
            }
            walk_children(ctx, st, left);
        }
        "object_pattern" | "array_pattern" => declare_pattern(ctx, left),
        _ => walk(ctx, st, left),
    }
}

fn call(ctx: &mut Ctx, st: &mut State, node: Node) {
    // An element hint is for a callback passed directly to the call that set it.
    ctx.lambda_hint = None;
    let Some(func) = node.child_by_field_name("function") else {
        walk_children(ctx, st, node);
        return;
    };
    let args = node.child_by_field_name("arguments");
    let argc = args.map(count_args);
    let line = line_of(node);
    let type_args = node.child_by_field_name("type_arguments").map(|t| ctx.text(t).to_string());
    // `it.each(rows)('adds %i', fn)` / `describe.each(table)(…)`.
    if func.kind() == "call_expression" {
        if let Some(base) = each_test_base(ctx, func) {
            let arg_list: Vec<Node> = args.map(named_children).unwrap_or_default();
            if test_block(ctx, st, node, &base, &arg_list) {
                walk(ctx, st, func);
                return;
            }
        }
    }
    // Vite `import.meta.glob('./pages/*.tsx', { eager: true })`: the matching modules are imported.
    if ctx.text(func).starts_with("import.meta.glob") {
        let patterns: Vec<String> = match args.and_then(|a| a.named_child(0)) {
            Some(a) if a.kind() == "array" => named_children(a).into_iter().filter_map(|s| string_like(s, ctx.src)).collect(),
            Some(a) => string_like(a, ctx.src).into_iter().collect(),
            None => Vec::new(),
        };
        for p in patterns.into_iter().filter(|p| !p.starts_with('!')) {
            ctx.facts.imports.push(Import { module: p, imported: Some("default".into()), local: None, kind: ImportKind::Default, line });
        }
        return;
    }
    // `super(…)`: the base class constructor.
    if func.kind() == "super" {
        if st.phaser {
            phaser_super(ctx, args);
        }
        ctx.push_ref(RefKind::Call, "constructor", Some("super".into()), None, argc, line, None);
        if let Some(a) = args {
            walk(ctx, st, a);
        }
        return;
    }
    // `setUser(…)` (React state) / `set(…)` inside a store: a state mutation.
    if func.kind() == "identifier" {
        let fname = ctx.text(func);
        if let Some(state) = st.setters.get(fname).cloned() {
            ctx.push_mutation(&state, "component_state", line_of(node));
        } else if let (Some(store), "set" | "setState" | "patch") = (st.store.clone(), fname) {
            ctx.push_mutation(&store, "store_state", line_of(node));
        }
    }
    // TanStack Router `createFileRoute('/posts')({ component: Posts })`.
    if func.kind() == "call_expression" {
        let inner = func.child_by_field_name("function").map(|f| ctx.text(f)).unwrap_or("");
        if inner == "createFileRoute" || inner == "createLazyFileRoute" {
            let path = func.child_by_field_name("arguments").and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
            let comp = node.child_by_field_name("arguments").and_then(|a| a.named_child(0)).and_then(|o| object_prop(ctx, o, "component"));
            if let (Some(path), Some(comp)) = (path, comp.filter(|c| c.kind() == "identifier")) {
                let target = ctx.text(comp).to_string();
                let enclosing = ctx.enclosing();
                ctx.push_route(&format!("ref:{target}|{enclosing}"), "PAGE", &path, "tanstack-router", line_of(node));
            }
        }
    }
    // Calling a local (parameter, local arrow function) is not a call to a declared symbol.
    if func.kind() == "identifier" && ctx.is_local(ctx.text(func)) {
        if let Some(a) = args {
            walk(ctx, st, a);
        }
        return;
    }
    let (name, recv_node) = match func.kind() {
        "identifier" => (ctx.text(func).to_string(), None),
        "member_expression" => {
            (func.child_by_field_name("property").map(|p| ctx.text(p).to_string()).unwrap_or_default(), func.child_by_field_name("object"))
        }
        "import" => {
            // dynamic import("./x")
            // Lazy loading binds no names (never a re-export of the target's members).
            if let Some((m, _)) = args.and_then(|a| first_string_arg(a, ctx.src)) {
                ctx.facts.imports.push(Import { module: m, imported: None, local: None, kind: ImportKind::SideEffect, line });
            }
            return;
        }
        _ => {
            walk(ctx, st, func);
            if let Some(a) = args {
                walk(ctx, st, a);
            }
            return;
        }
    };
    let (receiver, rtype) = match recv_node {
        Some(r) => {
            let (t, ty) = receiver_info(ctx, r);
            (Some(t), ty)
        }
        None => (None, None),
    };
    let arg_list: Vec<Node> = args.map(named_children).unwrap_or_default();

    // ---- Test blocks: describe("x", () => {...}) / it("y", ...) ----
    if receiver.is_none() || receiver.as_deref().map(|r| TEST_FNS.contains(&r)).unwrap_or(false) {
        let base = if receiver.is_none() { name.clone() } else { receiver.clone().unwrap() };
        if TEST_FNS.contains(&base.as_str()) && test_block(ctx, st, node, &base, &arg_list) {
            return;
        }
    }

    let recv_lower = receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let recv_last = last_segment(&recv_lower).trim_start_matches('_').trim_start_matches('$').to_string();
    let first_str = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
    let mut handled_args: Vec<Node> = Vec::new();

    // ---- Server routes (express-like) ----
    let lname = name.to_ascii_lowercase();
    let is_route_verb = HTTP_VERBS.contains(&lname.as_str()) || lname == "all";
    // `router.route('/x').get(h)`: the path comes from the `route()` call.
    let chained_path = recv_node.filter(|r| r.kind() == "call_expression").and_then(|r| {
        let f = r.child_by_field_name("function")?;
        let prop = f.child_by_field_name("property").map(|p| ctx.text(p))?;
        (prop == "route").then(|| r.child_by_field_name("arguments").and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s))?
    });
    let rtype_base = rtype.as_deref().filter(|t| !t.starts_with("@mt#")).map(clean_type).unwrap_or_default();
    let routerish = recv_last == "app"
        || recv_last == "router"
        || recv_last == "server"
        || recv_last == "fastify"
        || recv_last.ends_with("router")
        || recv_last == "routes"
        || recv_last == "api" && st.server_file
        || ROUTER_TYPES.contains(&rtype_base.as_str())
        || chained_path.is_some();
    // The router variable the route is declared on (`router` in `router.route('/x').get(…)`).
    let recv_base = receiver.as_deref().unwrap_or("").split(['.', '(']).next().unwrap_or("").to_string();
    let framework = match rtype_base.as_str() {
        "Hono" => "hono",
        "FastifyInstance" => "fastify",
        "Koa" | "KoaRouter" => "koa",
        _ => st.framework.unwrap_or("express"),
    };
    let min_args = if chained_path.is_some() { 1 } else { 2 };
    let mut is_server_route = false;
    if st.server_file && is_route_verb && routerish && arg_list.len() >= min_args {
        let path = chained_path.clone().or_else(|| first_str.clone());
        if let Some(path) = path.filter(|p| p.starts_with('/') || p == "*") {
            is_server_route = true;
            let method = if lname == "all" { "ANY".to_string() } else { lname.to_ascii_uppercase() };
            let handler = *arg_list.last().unwrap();
            handled_args.extend(server_route(ctx, st, framework, &method, &path, handler, &recv_base, line));
        }
    }
    // `fastify.route({ method: 'POST', url: '/x', handler })`.
    if st.server_file && name == "route" && routerish && arg_list.len() == 1 {
        let opts = arg_list[0];
        let url = object_string_prop(ctx, opts, "url").or_else(|| object_string_prop(ctx, opts, "path"));
        let handler = object_prop(ctx, opts, "handler");
        if let (Some(url), Some(handler)) = (url.filter(|u| u.starts_with('/')), handler) {
            is_server_route = true;
            let method = object_string_prop(ctx, opts, "method").unwrap_or_else(|| "ANY".into()).to_ascii_uppercase();
            if server_route(ctx, st, framework, &method, &url, handler, &recv_base, line).is_some() {
                // The options object is still walked (other keys); the handler body is done.
                st.walked_handlers.push(handler.id());
            }
        }
    }
    // Fastify lifecycle hooks: `fastify.addHook('onRequest', async (request, reply) => …)`.
    if st.server_file && name == "addHook" && routerish {
        if let (Some(hook), Some(handler)) = (first_str.clone(), arg_list.get(1).copied().filter(|h| is_function_value(*h))) {
            let label = format!("hook {hook}");
            handler_symbol(ctx, st, framework, &label, &format!("{framework} hook {hook}"), handler);
            handled_args.push(handler);
        }
    }
    // `fastify.decorate('db', new Db())` / `decorateRequest('user', null)`: members every
    // instance / request / reply has; a decorated function is a symbol of its own.
    if st.server_file && matches!(name.as_str(), "decorate" | "decorateRequest" | "decorateReply") {
        if let (Some(member), Some(value)) = (first_str.clone(), arg_list.get(1).copied()) {
            let owner = match name.as_str() {
                "decorateRequest" => "FastifyRequest",
                "decorateReply" => "FastifyReply",
                _ => "FastifyInstance",
            };
            let ty = if is_function_value(value) {
                let before = ctx.facts.symbols.len();
                visit_function_like(ctx, st, &member, value, value, false, false, Vec::new());
                if let Some(f) = ctx.facts.symbols.get_mut(before) {
                    f.signature = format!("{owner}.{}", f.signature);
                    f.attrs.push(format!("{name}('{member}')"));
                }
                handled_args.push(value);
                Some("@fn".to_string())
            } else {
                infer_value_type(ctx, value).filter(|t| !t.starts_with("@mt#"))
            };
            if let Some(ty) = ty {
                ctx.facts.route_prefixes.push(format!("decorate:{owner}|{member}|{ty}"));
            }
        }
    }
    // `fastify.register(autoload, { dir: path.join(__dirname, 'routes'), options: { prefix } })`:
    // each folder below `dir` prefixes the routes of the plugins in it.
    if st.server_file && name == "register" && arg_list.first().map(|a| ctx.text(*a).to_ascii_lowercase().contains("autoload")).unwrap_or(false) {
        if let Some(opts) = arg_list.get(1).copied() {
            let dir = object_prop(ctx, opts, "dir").map(|d| string_args(ctx, d).join("/"));
            let prefix = object_prop(ctx, opts, "options").and_then(|o| object_string_prop(ctx, o, "prefix")).unwrap_or_default();
            if let Some(dir) = dir.filter(|d| !d.is_empty()) {
                let base = ctx.path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let full = normalize_rel(&format!("{base}/{dir}"));
                ctx.facts.route_prefixes.push(format!("autoload:{prefix}|{full}"));
            }
        }
    }
    // `app.use('/api', usersRouter)` / `fastify.register(routes, { prefix: '/v1' })`: every route
    // of the mounted router is served under the prefix.
    if st.server_file && routerish && !is_route_verb {
        let mount = match name.as_str() {
            "use" => first_str.clone().filter(|p| p.starts_with('/')).map(|p| (p, arg_list.iter().skip(1).copied().collect::<Vec<_>>())),
            "register" => arg_list
                .get(1)
                .and_then(|o| object_string_prop(ctx, *o, "prefix"))
                .filter(|p| p.starts_with('/'))
                .map(|p| (p, arg_list.first().copied().into_iter().collect())),
            _ => None,
        };
        if let Some((prefix, targets)) = mount {
            for t in targets {
                let target = match t.kind() {
                    "identifier" => Some(ctx.text(t).to_string()),
                    "call_expression" | "await_expression" => mount_module(ctx, t).map(|m| format!("module:{m}")),
                    _ => None,
                };
                if let Some(target) = target {
                    st.mounts.push(Mount { prefix: prefix.clone(), target, parent: recv_base.clone() });
                }
            }
        }
    }

    // ---- Frontend routes ----
    // `createBrowserRouter([{ path, element, children }])`, `useRoutes([…])`, Vue
    // `createRouter({ routes })`, TanStack `createRoute({ path, component })`.
    let router_fn = matches!(name.as_str(), "createBrowserRouter" | "createHashRouter" | "createMemoryRouter" | "useRoutes" | "createRouter");
    if router_fn {
        let framework = if name == "createRouter" { "vue-router" } else { "react-router" };
        if let Some(a) = arg_list.first().copied() {
            let routes = match a.kind() {
                "array" => Some(a),
                "object" => object_prop(ctx, a, "routes").and_then(|r| array_value(ctx, r)),
                "identifier" => array_value(ctx, a),
                _ => None,
            };
            if let Some(routes) = routes {
                route_objects(ctx, routes, "", framework);
            }
        }
    }
    if name == "createRoute" {
        if let Some(o) = arg_list.first().copied() {
            if let (Some(path), Some(comp)) = (object_string_prop(ctx, o, "path"), object_prop(ctx, o, "component").filter(|c| c.kind() == "identifier")) {
                let target = ctx.text(comp).to_string();
                let enclosing = ctx.enclosing();
                ctx.push_route(&format!("ref:{target}|{enclosing}"), "PAGE", &path, "tanstack-router", line);
            }
        }
    }

    // ---- Playwright ----
    if st.playwright {
        match name.as_str() {
            // `page.goto('/users/1')`: the test visits a frontend page.
            "goto" => {
                if let Some(url) = first_str.clone() {
                    ctx.push_http_call("PAGE", &url, "playwright", line);
                }
            }
            // `page.waitForResponse('**/api/users')`: the page calls that API.
            "waitForResponse" | "waitForRequest" => {
                if let Some(url) = first_str.clone().filter(|u| u.contains('/')) {
                    ctx.push_http_call("", u_trim_glob(&url), "playwright", line);
                }
            }
            // `page.getByTestId('save')` / `locator('[data-testid="save"]')`.
            "getByTestId" => {
                if let (Some(id), Some(a)) = (first_str.clone(), arg_list.first()) {
                    ctx.push_literal(&id, "testid-query", *a);
                }
            }
            "locator" | "$" | "$$" | "waitForSelector" => {
                if let (Some(sel), Some(a)) = (first_str.clone(), arg_list.first()) {
                    for id in testids_in_selector(&sel) {
                        ctx.push_literal(&id, "testid-query", *a);
                    }
                }
            }
            _ => {}
        }
    }

    // ---- HTTP client calls ----
    // `app.inject({ method: 'GET', url: '/users' })` (Fastify) and `app.inject().get('/users')`.
    if name == "inject" && (st.server_file || is_test_path(ctx.path)) {
        if let Some(opts) = arg_list.first() {
            if let Some(url) = object_string_prop(ctx, *opts, "url").or_else(|| object_string_prop(ctx, *opts, "path")) {
                let method = object_string_prop(ctx, *opts, "method").unwrap_or_else(|| "GET".into());
                ctx.push_http_call(&method.to_ascii_uppercase(), &url, "fastify.inject", line);
            }
        }
    }
    // supertest: `request(app).get('/users')`, `agent.post('/x')`, `app.inject().get('/x')`.
    let test_client = receiver
        .as_deref()
        .map(|r| r.starts_with("request(") || r.starts_with("supertest(") || r.ends_with("inject()") || r == "agent")
        .unwrap_or(false);
    if test_client && HTTP_VERBS.contains(&lname.as_str()) {
        if let Some(url) = first_str.clone() {
            ctx.push_http_call(&lname.to_ascii_uppercase(), &url, "supertest", line);
            is_server_route = true;
        }
    }
    if !is_server_route {
        if receiver.is_none() && matches!(name.as_str(), "fetch" | "$fetch" | "ofetch" | "useFetch") {
            if let Some(url) = first_str.clone() {
                let method = arg_list.get(1).and_then(|o| object_string_prop(ctx, *o, "method")).unwrap_or_else(|| "GET".into());
                ctx.push_http_call(&method, &url, "fetch", line);
            }
        } else if receiver.as_deref().map(|r| r.ends_with("window") || r == "globalThis").unwrap_or(false) && name == "fetch" {
            if let Some(url) = first_str.clone() {
                ctx.push_http_call("GET", &url, "fetch", line);
            }
        } else if HTTP_VERBS.contains(&lname.as_str()) || lname == "request" {
            let clientish = recv_last.contains("axios")
                || recv_last.contains("http")
                || recv_last.contains("api")
                || recv_last.contains("client")
                || recv_last == "ky"
                || recv_last == "request"
                || recv_last == "instance"
                || recv_last == "superagent";
            if clientish {
                let url = first_str.clone().or_else(|| arg_list.first().and_then(|o| object_string_prop(ctx, *o, "url")));
                if let Some(url) = url.filter(|u| looks_like_url(u)) {
                    let method = if lname == "request" {
                        arg_list.first().and_then(|o| object_string_prop(ctx, *o, "method")).unwrap_or_else(|| "GET".into())
                    } else {
                        lname.to_ascii_uppercase()
                    };
                    ctx.push_http_call(&method, &url, receiver.as_deref().unwrap_or("http"), line);
                }
            }
        } else if receiver.is_none() && name == "axios" {
            if let Some(cfg) = arg_list.first() {
                let url = string_like(*cfg, ctx.src).or_else(|| object_string_prop(ctx, *cfg, "url"));
                if let Some(url) = url {
                    let method = object_string_prop(ctx, *cfg, "method").unwrap_or_else(|| "GET".into());
                    ctx.push_http_call(&method, &url, "axios", line);
                }
            }
        }
    }

    // ---- Config, DI-adjacent and entrypoint patterns ----
    if matches!(name.as_str(), "get" | "getOrThrow") && recv_last.contains("config") {
        if let Some(k) = first_str.clone() {
            ctx.push_config(&k, "config_service", line, arg_list.get(1).map(|n| ctx.text(*n).to_string()));
        }
    }
    if name == "setGlobalPrefix" {
        if let Some(p) = first_str.clone() {
            ctx.facts.route_prefixes.push(p);
        }
    }
    if (name == "listen" && (recv_last == "app" || recv_last == "server" || recv_last == "fastify"))
        || (name == "create" && recv_last == "nestfactory")
    {
        let enclosing = ctx.enclosing();
        if let Some(s) = ctx.symbol_mut(&enclosing) {
            if !s.is_entrypoint {
                s.is_entrypoint = true;
                s.entry_kind = Some("server start".into());
            }
        }
    }
    if name == "command" && (recv_last == "program" || recv_last.contains("cli") || recv_last == "yargs") {
        let enclosing = ctx.enclosing();
        if let Some(s) = ctx.symbol_mut(&enclosing) {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("cli command {}", first_str.clone().unwrap_or_default()));
        }
    }

    // ---- State mutations ----
    // ---- Phaser: assets, animations, scenes, registry ----
    if st.phaser {
        phaser_call(ctx, &name, &recv_last, &arg_list, first_str.as_deref(), line);
    }
    // ---- Events: `emit('score')` ↔ `on('score', this.onScore)` ----
    if let Some(key) = first_str.clone().filter(|k| !k.is_empty()) {
        match name.as_str() {
            "emit" => {
                if let Some(a) = arg_list.first() {
                    ctx.push_literal(&key, "event-emit", *a);
                }
            }
            "on" | "once" | "addListener" => {
                let handler = arg_list.get(1).and_then(|h| callback_name(ctx, *h));
                let value = match handler {
                    Some(h) => format!("{key}#{h}"),
                    None => key.clone(),
                };
                if let Some(a) = arg_list.first() {
                    ctx.push_literal(&value, "event-listen", *a);
                }
            }
            _ => {}
        }
    }
    // ---- Callbacks the engine / emitter calls later: `collider(a, b, this.onHit)`,
    // `delayedCall(500, this.spawn)`, `tweens.add({ onComplete: this.done })` ----
    let mut callbacks: Vec<Node> = crate::phaser::callback_args(&name).iter().filter_map(|i| arg_list.get(*i).copied()).collect();
    if st.phaser {
        for o in arg_list.iter().filter(|a| a.kind() == "object") {
            callbacks.extend(crate::phaser::CALLBACK_KEYS.iter().filter_map(|k| object_prop(ctx, *o, k)));
        }
    }
    for cb in callbacks {
        if let Some(target) = callback_name(ctx, cb) {
            let this_member = cb.kind() == "member_expression";
            if this_member {
                let cls = ctx.class_name();
                ctx.push_ref(RefKind::Call, &target, Some("this".into()), cls, None, line_of(cb), Some(format!("callback via {name}")));
            } else if !ctx.is_local(&target) {
                ctx.push_ref(RefKind::Call, &target, None, None, None, line_of(cb), Some(format!("callback via {name}")));
            }
        }
    }

    let rtype_l = rtype.as_deref().unwrap_or("").to_ascii_lowercase();
    match name.as_str() {
        "create" | "createMany" | "update" | "updateMany" | "upsert" | "delete" | "deleteMany" if recv_lower.contains("prisma") => {
            let model = last_segment(receiver.as_deref().unwrap_or("")).to_string();
            ctx.push_mutation(&model, "database", line);
        }
        "save" | "insert" | "update" | "delete" | "remove" | "softDelete" | "upsert" | "increment" | "decrement"
            if recv_last.contains("repo")
                || recv_last.contains("repository")
                || recv_last.contains("manager")
                || rtype_l.contains("repository") =>
        {
            let ent =
                rtype.as_deref().and_then(|t| generic_args(t).into_iter().next()).unwrap_or_else(|| receiver.clone().unwrap_or_default());
            ctx.push_mutation(&ent, "database", line);
        }
        "create" | "insertMany" | "updateOne" | "updateMany" | "deleteOne" | "deleteMany" | "findByIdAndUpdate" | "findOneAndUpdate"
        | "findByIdAndDelete" | "replaceOne" | "insertOne"
            if receiver
                .as_deref()
                .map(|r| {
                    (is_upper_ident(last_segment(r)) && !is_non_model_receiver(last_segment(r)))
                        || r.contains("collection")
                        || r.contains("db.")
                })
                .unwrap_or(false) =>
        {
            ctx.push_mutation(last_segment(receiver.as_deref().unwrap()), "database", line);
        }
        "set" | "del" | "hset" | "setex" | "expire" | "put" | "invalidate" | "mset" | "setItem" | "removeItem" | "clear"
            if recv_lower.contains("cache") || recv_lower.contains("redis") || recv_lower.contains("storage") =>
        {
            let target = first_str.clone().map(|k| format!("cache:{k}")).unwrap_or_else(|| receiver.clone().unwrap_or_default());
            ctx.push_mutation(&target, "cache", line);
        }
        _ => {}
    }

    // `users.map(u => u.name)`: the callback's parameter is an element of the receiver.
    let pw_hook = st.playwright
        && receiver.as_deref().map(|r| TEST_FNS.contains(&r)).unwrap_or(false)
        && matches!(name.as_str(), "beforeEach" | "afterEach" | "beforeAll" | "afterAll" | "step");
    let hint = match (name.as_str(), rtype.as_deref()) {
        _ if pw_hook => Some(member_type_marker(Some("@fn"), receiver.as_deref().unwrap_or("test"), false)),
        // `map.forEach((user) => …)` on `Map<K, V>`: the value.
        ("forEach", Some(t)) if matches!(clean_type(t).as_str(), "Map" | "ReadonlyMap") => generic_args(t).get(1).cloned(),
        (m, Some(t)) if ELEMENT_CALLBACK_METHODS.contains(&m) => ts_element_type(t),
        // `loadUser().then((u) => …)`: the resolved value.
        ("then", Some(t)) => ts_awaited_type(t),
        _ => None,
    };
    let detail = type_args.map(|t| collapse_ws(&t));
    ctx.push_ref(RefKind::Call, &name, receiver, rtype, argc, line, detail);
    if let Some(r) = recv_node {
        walk(ctx, st, r);
    }
    for a in arg_list {
        if !handled_args.contains(&a) {
            ctx.lambda_hint = if is_function_value(a) { hint.clone() } else { None };
            walk(ctx, st, a);
            ctx.lambda_hint = None;
        }
    }
}

/// `**/api/users` (a Playwright URL glob) → `/api/users`.
fn u_trim_glob(url: &str) -> &str {
    let t = url.trim_start_matches('*');
    if t.starts_with('/') {
        t
    } else {
        url
    }
}

/// `[data-testid="save"]` / `[data-test=save] button` → [`save`].
fn testids_in_selector(sel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for attr in TESTID_ATTRS {
        let mut rest = sel;
        while let Some(i) = rest.find(&format!("[{attr}=")) {
            rest = &rest[i + attr.len() + 2..];
            let v: String = rest.trim_start_matches(['"', '\'']).chars().take_while(|c| !matches!(c, '"' | '\'' | ']')).collect();
            if !v.is_empty() {
                out.push(v);
            }
        }
    }
    out
}

/// The array a router config reads: a literal, or a top-level `const routes = [ … ]` named by it.
fn array_value<'t>(ctx: &Ctx, n: Node<'t>) -> Option<Node<'t>> {
    match n.kind() {
        "array" => Some(n),
        "identifier" | "shorthand_property_identifier" => {
            let name = ctx.text(n);
            let mut root = n;
            while let Some(p) = root.parent() {
                root = p;
            }
            let mut stack = vec![root];
            while let Some(x) = stack.pop() {
                if x.kind() == "variable_declarator" && x.child_by_field_name("name").map(|d| ctx.text(d) == name).unwrap_or(false) {
                    let v = x.child_by_field_name("value")?;
                    let v = if matches!(v.kind(), "as_expression" | "satisfies_expression") { v.named_child(0)? } else { v };
                    return (v.kind() == "array").then_some(v);
                }
                if !is_function_value(x) {
                    stack.extend(named_children(x));
                }
            }
            None
        }
        _ => None,
    }
}

/// Route objects `{ path, element | component | Component, children, index }` (React Router,
/// Vue Router, Angular): each page component is reached at its full path.
fn route_objects(ctx: &mut Ctx, routes: Node, parent: &str, framework: &str) {
    for o in named_children(routes).into_iter().filter(|o| o.kind() == "object") {
        let own = object_string_prop(ctx, o, "path");
        let path = match own.as_deref() {
            Some(p) if p.starts_with('/') => p.to_string(),
            Some(p) => mount_join(parent, p),
            None => mount_join(parent, ""),
        };
        let comp = ["element", "component", "Component"].iter().find_map(|k| object_prop(ctx, o, k)).and_then(|v| match v.kind() {
            "identifier" => Some(ctx.text(v).to_string()),
            "jsx_self_closing_element" => v.child_by_field_name("name").map(|n| ctx.text(n).to_string()),
            "jsx_element" => v.child_by_field_name("open_tag").and_then(|t| t.child_by_field_name("name")).map(|n| ctx.text(n).to_string()),
            _ => None,
        });
        if let Some(comp) = comp.filter(|c| is_upper_ident(last_segment(c))) {
            let enclosing = ctx.enclosing();
            ctx.push_route(&format!("ref:{}|{enclosing}", last_segment(&comp)), "PAGE", &path, framework, line_of(o));
        }
        if let Some(children) = object_prop(ctx, o, "children").and_then(|c| array_value(ctx, c)) {
            route_objects(ctx, children, &path, framework);
        }
    }
}

/// `<Route path="users" element={<Users />}>` (nested in parent `<Route>`s): a page route.
fn jsx_route(ctx: &mut Ctx, opening: Node) {
    let attr = |el: Node, name: &str| -> Option<String> {
        named_children(el).into_iter().filter(|a| a.kind() == "jsx_attribute").find_map(|a| {
            let kids = named_children(a);
            (kids.first().map(|k| ctx.text(*k)) == Some(name)).then(|| kids.get(1).copied()).flatten().map(|v| {
                let v = if v.kind() == "jsx_expression" { v.named_child(0).unwrap_or(v) } else { v };
                string_like(v, ctx.src).unwrap_or_else(|| ctx.text(v).to_string())
            })
        })
    };
    let element_of = |el: Node| -> Option<String> {
        let a = named_children(el).into_iter().filter(|a| a.kind() == "jsx_attribute").find(|a| {
            named_children(*a).first().map(|k| matches!(ctx.text(*k), "element" | "component" | "Component")).unwrap_or(false)
        })?;
        let v = named_children(a).get(1).copied()?;
        let v = if v.kind() == "jsx_expression" { v.named_child(0)? } else { v };
        match v.kind() {
            "identifier" => Some(ctx.text(v).to_string()),
            "jsx_self_closing_element" => v.child_by_field_name("name").map(|n| ctx.text(n).to_string()),
            "jsx_element" => v.child_by_field_name("open_tag").and_then(|t| t.child_by_field_name("name")).map(|n| ctx.text(n).to_string()),
            _ => None,
        }
    };
    let Some(comp) = element_of(opening) else { return };
    // Segments of the enclosing `<Route>`s, innermost last.
    let mut segs = vec![attr(opening, "path").unwrap_or_default()];
    let mut cur = opening.parent();
    if opening.kind() == "jsx_opening_element" {
        cur = cur.and_then(|p| p.parent());
    }
    while let Some(n) = cur {
        if n.kind() == "jsx_element" {
            if let Some(open) = n.child_by_field_name("open_tag").filter(|t| t.child_by_field_name("name").map(|x| ctx.text(x) == "Route").unwrap_or(false)) {
                let p = attr(open, "path").unwrap_or_default();
                segs.insert(0, p);
            }
        }
        cur = n.parent();
    }
    let mut path = String::new();
    for s in segs {
        path = if s.starts_with('/') { s } else { mount_join(&path, &s) };
    }
    let path = if path.is_empty() { "/".to_string() } else { path };
    let enclosing = ctx.enclosing();
    ctx.push_route(&format!("ref:{}|{enclosing}", last_segment(&comp)), "PAGE", &path, "react-router", line_of(opening));
}

/// `this.onHit` / `this.onHit.bind(this)` / `onHit` passed as a callback → `onHit`.
fn callback_name(ctx: &Ctx, n: Node) -> Option<String> {
    match n.kind() {
        "identifier" => Some(ctx.text(n).to_string()),
        "member_expression" if n.child_by_field_name("object").map(|o| o.kind() == "this").unwrap_or(false) => {
            n.child_by_field_name("property").map(|p| ctx.text(p).to_string())
        }
        // `this.onHit.bind(this)`
        "call_expression" => {
            let f = n.child_by_field_name("function").filter(|f| f.kind() == "member_expression")?;
            if f.child_by_field_name("property").map(|p| ctx.text(p)) != Some("bind") {
                return None;
            }
            callback_name(ctx, f.child_by_field_name("object")?)
        }
        _ => None,
    }
}

/// A key defined in code (`this.load.image('player', url)`, `anims.create({ key })`).
fn push_config_define(ctx: &mut Ctx, key: &str, source: &str, line: u32, value: Option<String>) {
    let enclosing = Some(ctx.enclosing());
    ctx.facts.config.push(ConfigDep { key: key.to_string(), source_type: source.to_string(), line, default_value: value, usage: "define".into(), enclosing });
}

/// `super('Game')` / `super({ key: 'Game' })` in a scene names it; `super(scene, x, y, 'player')`
/// in a game object uses a texture.
fn phaser_super(ctx: &mut Ctx, args: Option<Node>) {
    let Some(args) = args else { return };
    let list = named_children(args);
    let key = match list.first() {
        Some(a) if a.kind() == "string" => Some((string_value(*a, ctx.src), *a)),
        Some(a) if a.kind() == "object" => object_prop(ctx, *a, "key").filter(|k| k.kind() == "string").map(|k| (string_value(k, ctx.src), k)),
        _ => None,
    };
    if let (Some((key, node)), 1) = (key, list.len()) {
        // The scene is the class, not its constructor.
        let class_id = ctx.scopes.iter().rev().find(|s| s.kind == Some(SymbolKind::Class)).map(|s| s.symbol_id.clone());
        ctx.push_literal(&key, "scene-key", node);
        if let (Some(id), Some(lit)) = (class_id, ctx.facts.literals.last_mut()) {
            lit.enclosing = id;
        }
        return;
    }
    if let Some(tex) = list.get(3).filter(|a| a.kind() == "string") {
        let key = string_value(*tex, ctx.src);
        ctx.push_config(&format!("asset:{key}"), "phaser.texture", line_of(*tex), None);
    }
}

/// Phaser API calls inside scenes and game objects.
fn phaser_call(ctx: &mut Ctx, name: &str, recv_last: &str, args: &[Node], first: Option<&str>, line: u32) {
    let str_arg = |i: usize| args.get(i).and_then(|a| string_like(*a, ctx.src));
    // Loader: `this.load.image('player', 'assets/player.png')`.
    if recv_last == "load" {
        if let Some(kind) = crate::phaser::loader_kind(name) {
            if let Some(key) = first {
                let ns = if kind == "anim" { "anim" } else { "asset" };
                push_config_define(ctx, &format!("{ns}:{key}"), &format!("phaser.{kind}"), line, str_arg(1));
            } else if let Some(items) = args.first().filter(|a| a.kind() == "array") {
                // `this.load.image([{ key, url }, …])`
                for o in named_children(*items).into_iter().filter(|o| o.kind() == "object") {
                    if let Some(key) = object_string_prop(ctx, o, "key") {
                        let url = object_string_prop(ctx, o, "url");
                        push_config_define(ctx, &format!("asset:{key}"), &format!("phaser.{kind}"), line_of(o), url);
                    }
                }
            }
            return;
        }
    }
    // Textures used by factories: `this.add.sprite(x, y, 'player')`, `setTexture('x')`.
    if let Some(i) = crate::phaser::texture_arg(name) {
        let factory = matches!(recv_last, "add" | "make" | "anims" | "textures" | "staticgroup") || i == 0;
        if let Some(key) = str_arg(i).filter(|_| factory) {
            ctx.push_config(&format!("asset:{key}"), "phaser.texture", line, None);
        }
    }
    match (recv_last, name) {
        ("sound", "add" | "play" | "playAudioSprite") => {
            if let Some(key) = first {
                ctx.push_config(&format!("asset:{key}"), "phaser.audio", line, None);
            }
        }
        ("make" | "add", "tilemap") => {
            let key = first.map(String::from).or_else(|| args.first().and_then(|o| object_string_prop(ctx, *o, "key")));
            if let Some(key) = key {
                ctx.push_config(&format!("asset:{key}"), "phaser.tilemap", line, None);
            }
        }
        (_, "addTilesetImage") => {
            if let Some(key) = str_arg(1).or_else(|| first.map(String::from)) {
                ctx.push_config(&format!("asset:{key}"), "phaser.texture", line, None);
            }
        }
        ("json" | "xml" | "text" | "binary" | "tilemap" | "audio", "get") => {
            if let Some(key) = first {
                ctx.push_config(&format!("asset:{key}"), "phaser.data", line, None);
            }
        }
        // Animations: `this.anims.create({ key: 'walk', … })` ↔ `sprite.play('walk')`.
        ("anims", "create") => {
            if let Some(key) = args.first().and_then(|o| object_string_prop(ctx, *o, "key")) {
                push_config_define(ctx, &format!("anim:{key}"), "phaser.anim", line, None);
            }
        }
        (r, "play" | "playReverse" | "playAfterDelay" | "playAfterRepeat" | "chain") if r != "sound" => {
            let key = first.map(String::from).or_else(|| args.first().and_then(|o| object_string_prop(ctx, *o, "key")));
            if let Some(key) = key {
                ctx.push_config(&format!("anim:{key}"), "phaser.anim", line, None);
            }
        }
        // Scenes: `this.scene.start('Game', data)` runs the scene registered as 'Game'.
        ("scene", n) if crate::phaser::SCENE_TRANSITIONS.contains(&n) => {
            if let (Some(_), Some(a)) = (first, args.first()) {
                let key = first.unwrap().to_string();
                ctx.push_literal(&key, "scene-ref", *a);
            }
        }
        // `this.scene.add('Pause', PauseScene)`: a scene registered at runtime.
        ("scene", "add") => {
            if let (Some(key), Some(cls)) = (first, args.get(1).filter(|c| c.kind() == "identifier")) {
                let value = format!("{key}#{}", ctx.text(*cls));
                ctx.push_literal(&value, "scene-key", *cls);
            }
        }
        // Registry / data manager: shared game state.
        ("registry" | "data", "set" | "inc" | "toggle" | "remove") | (_, "setData" | "incData" | "toggleData") => {
            if let Some(key) = first {
                ctx.push_mutation(&format!("registry:{key}"), "game_registry", line);
                if matches!(name, "set" | "setData") {
                    push_config_define(ctx, &format!("registry:{key}"), "phaser.registry", line, args.get(1).map(|v| ctx.text(*v).to_string()));
                }
            }
        }
        ("registry" | "data", "get") | (_, "getData") => {
            if let Some(key) = first {
                ctx.push_config(&format!("registry:{key}"), "phaser.registry", line, None);
            }
        }
        _ => {}
    }
}

/// Receiver types that declare routes (`new Hono()`, `fastify()` typed instances, `Router`).
const ROUTER_TYPES: &[&str] = &["Hono", "Router", "Express", "Application", "FastifyInstance", "Koa", "KoaRouter", "Elysia"];

/// A route on a server / router: an inline handler becomes a `GET /x` function symbol, a named
/// one is resolved later. Returns the handler node when its body was walked here.
#[allow(clippy::too_many_arguments)]
fn server_route<'t>(
    ctx: &mut Ctx,
    st: &mut State,
    framework: &str,
    method: &str,
    path: &str,
    handler: Node<'t>,
    recv: &str,
    line: u32,
) -> Option<Node<'t>> {
    let walked = if is_function_value(handler) {
        let label = format!("{method} {path}");
        let id = handler_symbol(ctx, st, framework, &label, &format!("route {method} {path}"), handler);
        ctx.push_route(&id, method, path, framework, line);
        Some(handler)
    } else {
        let target = last_segment(ctx.text(handler)).to_string();
        let enclosing = ctx.enclosing();
        ctx.push_route(&format!("ref:{target}|{enclosing}"), method, path, framework, line);
        None
    };
    st.route_recv.push((ctx.facts.routes.len() - 1, recv.to_string()));
    walked
}

/// An inline route / hook handler: a `GET /x` (or `hook onRequest`) entrypoint symbol whose
/// body is walked here; Fastify handlers get typed `request` / `reply` parameters.
fn handler_symbol(ctx: &mut Ctx, st: &mut State, framework: &str, label: &str, entry: &str, handler: Node) -> String {
    let mut s = NewSymbol::new(SymbolKind::Function, label, ctx.child_qname(label), line_of(handler), end_line_of(handler));
    s.signature = format!("{label} handler");
    s.is_entrypoint = true;
    s.entry_kind = Some(entry.to_string());
    s.complexity = cyclomatic(handler, ctx.src, ctx.lang);
    s.body_hash = token_hash(handler, ctx.src);
    s.is_async = children(handler).iter().any(|c| c.kind() == "async");
    let id = ctx.add_symbol(s);
    ctx.push_scope(Scope { symbol_id: id.clone(), kind: Some(SymbolKind::Function), qname: ctx.child_qname(label), is_function: true, ..Default::default() });
    let (_, _, mut locals, _) = parse_params(ctx, handler.child_by_field_name("parameters").or_else(|| handler.child_by_field_name("parameter")));
    if framework == "fastify" {
        for ((_, t), default) in locals.iter_mut().zip(["FastifyRequest", "FastifyReply"]) {
            if t.is_none() {
                *t = Some(default.to_string());
            }
        }
    }
    for (n, t) in locals {
        ctx.declare_local(&n, t);
    }
    if let Some(b) = handler.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    ctx.pop_scope();
    id
}

/// String arguments of a path expression: `path.join(__dirname, 'routes')` → [`routes`].
fn string_args(ctx: &Ctx, n: Node) -> Vec<String> {
    if let Some(s) = string_like(n, ctx.src) {
        return vec![s];
    }
    named_children(n).into_iter().flat_map(|c| string_args(ctx, c)).collect()
}

fn normalize_rel(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

/// Fastify plugins (`fp(async (fastify, opts) => …)`, `export default async function (app)`)
/// take the instance first: untyped `fastify` / `app` / `server` / `instance` parameters of a
/// Fastify file are a `FastifyInstance`, `request` / `reply` its request and reply.
fn fastify_param_types(st: &State, locals: &mut [(String, Option<String>)]) {
    if st.framework != Some("fastify") {
        return;
    }
    for (n, t) in locals.iter_mut().filter(|(_, t)| t.is_none()) {
        *t = match n.as_str() {
            "fastify" | "app" | "server" | "instance" => Some("FastifyInstance".into()),
            "request" => Some("FastifyRequest".into()),
            "reply" => Some("FastifyReply".into()),
            _ => None,
        };
    }
}

/// `require('./users')` / `import('./users')` passed straight to `app.use(…)`.
fn mount_module(ctx: &Ctx, n: Node) -> Option<String> {
    let call = if n.kind() == "await_expression" { n.named_child(0)? } else { n };
    let f = call.child_by_field_name("function")?;
    if !matches!(f.kind(), "import" | "identifier") || (f.kind() == "identifier" && ctx.text(f) != "require") {
        return None;
    }
    first_string_arg(call.child_by_field_name("arguments")?, ctx.src).map(|(s, _)| s)
}

/// `{ key: value }` / `{ key }` property of an object literal.
fn object_prop<'t>(ctx: &Ctx, obj: Node<'t>, key: &str) -> Option<Node<'t>> {
    if obj.kind() != "object" {
        return None;
    }
    named_children(obj).into_iter().find_map(|p| match p.kind() {
        "pair" if p.child_by_field_name("key").map(|k| ctx.text(k).trim_matches(['"', '\'']) == key).unwrap_or(false) => {
            p.child_by_field_name("value")
        }
        "shorthand_property_identifier" if ctx.text(p) == key => Some(p),
        "method_definition" if member_name(ctx, p) == key => Some(p),
        _ => None,
    })
}

/// `/api` + `/users` → `/api/users` (a mounted router's paths are relative to its mount point).
fn mount_join(prefix: &str, path: &str) -> String {
    let p = prefix.trim_matches('/');
    let s = path.trim_start_matches('/');
    match (p.is_empty(), s.is_empty()) {
        (true, true) => "/".into(),
        (true, false) => format!("/{s}"),
        (false, true) => format!("/{p}"),
        (false, false) => format!("/{p}/{s}"),
    }
}

/// Full mount prefix of a router variable (`app.use('/api', v1); v1.use('/users', users)`).
fn mount_prefix(st: &State, var: &str, depth: u32) -> Option<String> {
    if depth > 6 || var.is_empty() {
        return None;
    }
    let m = st.mounts.iter().find(|m| m.target == var)?;
    let parent = mount_prefix(st, &m.parent, depth + 1).unwrap_or_default();
    Some(mount_join(&parent, &m.prefix))
}

/// Prefixes the routes of routers mounted in this file; mounts of imported routers are left
/// to the cross-file pass (`mount:<prefix>|<module>` route prefixes).
fn apply_mounts(ctx: &mut Ctx, st: &State) {
    for (idx, recv) in &st.route_recv {
        let Some(prefix) = mount_prefix(st, recv, 0) else { continue };
        let Some(r) = ctx.facts.routes.get_mut(*idx) else { continue };
        r.path = mount_join(&prefix, &r.path);
        let (handler, entry) = (r.handler.clone(), format!("route {} {}", r.http_method, r.path));
        if let Some(s) = ctx.symbol_mut(&handler) {
            s.entry_kind = Some(entry);
        }
    }
    let local_routers: std::collections::HashSet<&str> = st.route_recv.iter().map(|(_, r)| r.as_str()).collect();
    for m in &st.mounts {
        if local_routers.contains(m.target.as_str()) {
            continue;
        }
        let module = match m.target.strip_prefix("module:") {
            Some(spec) => Some(spec.to_string()),
            None => ctx.facts.imports.iter().find(|i| i.local.as_deref() == Some(m.target.as_str())).map(|i| i.module.clone()),
        };
        if let Some(module) = module {
            let parent = mount_prefix(st, &m.parent, 0).unwrap_or_default();
            ctx.facts.route_prefixes.push(format!("mount:{}|{module}", mount_join(&parent, &m.prefix)));
        }
    }
}

/// `X.each(table)` where X is a test function: returns X.
fn each_test_base(ctx: &Ctx, inner: Node) -> Option<String> {
    let f = inner.child_by_field_name("function")?;
    if f.kind() != "member_expression" {
        return None;
    }
    let prop = f.child_by_field_name("property").map(|p| ctx.text(p))?;
    let obj = f.child_by_field_name("object").map(|o| ctx.text(o).to_string())?;
    let base = obj.split('.').next().unwrap_or(&obj).to_string();
    (prop == "each" && TEST_FNS.contains(&base.as_str())).then_some(base)
}

/// `describe("label", () => …)` / `it(Service.name, async () => …)`: a Test symbol.
fn test_block(ctx: &mut Ctx, st: &mut State, node: Node, base: &str, arg_list: &[Node]) -> bool {
    if arg_list.len() < 2 {
        return false;
    }
    let Some(cb) = arg_list.iter().rev().find(|a| is_function_value(**a)).copied() else { return false };
    let label = string_like(arg_list[0], ctx.src).unwrap_or_else(|| collapse_ws(ctx.text(arg_list[0])));
    if label.is_empty() {
        return false;
    }
    let line = line_of(node);
    let mut s = NewSymbol::new(SymbolKind::Test, &label, ctx.child_qname(&truncate(&label, 60)), line, end_line_of(node));
    s.signature = format!("{base}(\"{}\")", truncate(&label, 80));
    s.is_test = true;
    s.complexity = cyclomatic(cb, ctx.src, ctx.lang);
    s.body_hash = token_hash(node, ctx.src);
    let id = ctx.add_symbol(s);
    if string_like(arg_list[0], ctx.src).is_some() {
        ctx.push_literal(&label, "string", arg_list[0]);
    } else {
        walk(ctx, st, arg_list[0]);
    }
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Test),
        qname: ctx.child_qname(&truncate(&label, 60)),
        is_function: true,
        ..Default::default()
    });
    let params = cb.child_by_field_name("parameters").or_else(|| cb.child_by_field_name("parameter"));
    let (_, _, mut locals, _) = parse_params(ctx, params);
    if st.playwright {
        // `test('…', async ({ page, todoPage }) => …)`: fixtures of the (possibly extended) `test`.
        apply_param_hint(ctx, params, &member_type_marker(Some("@fn"), base, false), &mut locals);
    }
    for (n, t) in locals {
        ctx.declare_local(&n, t);
    }
    if let Some(b) = cb.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    ctx.pop_scope();
    true
}

fn looks_like_url(u: &str) -> bool {
    u.starts_with('/') || u.starts_with("http") || u.starts_with("${") || (u.contains('/') && !u.contains(' '))
}

fn object_string_prop(ctx: &Ctx, obj: Node, key: &str) -> Option<String> {
    if obj.kind() != "object" {
        return None;
    }
    for p in named_children(obj) {
        if p.kind() == "pair" {
            let k = p.child_by_field_name("key").map(|k| ctx.text(k).trim_matches(|c| c == '"' || c == '\'').to_string());
            if k.as_deref() == Some(key) {
                return p.child_by_field_name("value").and_then(|v| string_like(v, ctx.src));
            }
        }
    }
    None
}

/// Upper-case receivers that are factories / builtins rather than ORM models.
fn is_non_model_receiver(r: &str) -> bool {
    r.ends_with("Factory")
        || r.ends_with("Builder")
        || matches!(
            r,
            "Object"
                | "Array"
                | "Promise"
                | "Date"
                | "Map"
                | "Set"
                | "Reflect"
                | "Proxy"
                | "URL"
                | "React"
                | "ReactDOM"
                | "Vue"
                | "JSON"
                | "Math"
                | "Test"
                | "Symbol"
        )
}
