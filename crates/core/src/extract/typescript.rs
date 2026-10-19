//! TypeScript / TSX / JavaScript extractor.

use tree_sitter::Node;

use super::*;
use crate::routes::join_route;

const SERVER_PACKAGES: &[&str] = &["express", "fastify", "koa", "@koa/router", "koa-router", "hono", "restify"];
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

#[derive(Default)]
struct State {
    server_file: bool,
    has_exports: bool,
}

#[derive(Clone, Default)]
struct ClassInfo {
    name: String,
    route_prefix: Option<String>,
    is_controller: bool,
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
    let mut st = State::default();
    for ch in named_children(root) {
        match ch.kind() {
            "import_statement" => {
                if let Some(src) = ch.child_by_field_name("source") {
                    let m = string_value(src, ctx.src);
                    if SERVER_PACKAGES.contains(&m.as_str()) {
                        st.server_file = true;
                    }
                }
            }
            "export_statement" => st.has_exports = true,
            _ => {
                let t = ctx.text(ch);
                if SERVER_PACKAGES.iter().any(|p| t.contains(&format!("require(\"{p}\")")) || t.contains(&format!("require('{p}')"))) {
                    st.server_file = true;
                }
                if t.contains("module.exports") || t.contains("exports.") {
                    st.has_exports = true;
                }
            }
        }
    }
    // Next.js route handlers: app/**/route.ts and pages/api/**.
    for ch in named_children(root) {
        visit_top(ctx, &mut st, ch, false, false, Vec::new());
    }
}

/// Records string literals inside a decorator expression (`@Get(':id')`, `@Controller('x')`).
fn literal_strings(ctx: &mut Ctx, n: Node) {
    if let Some(v) = string_like(n, ctx.src) {
        let t = if n.kind() == "template_string" { "template" } else { "string" };
        ctx.push_literal(&v, t, n);
        return;
    }
    for ch in named_children(n) {
        literal_strings(ctx, ch);
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
    // Only when the string is the first argument.
    if !rest[..q].trim().is_empty() {
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
                    decs.push(decorator_text(ctx, ch));
                    literal_strings(ctx, ch);
                }
                if ch.kind() == "default" {
                    default = true;
                }
            }
            if let Some(decl) = node.child_by_field_name("declaration") {
                visit_top(ctx, st, decl, true, default, decs);
            } else if let Some(src) = node.child_by_field_name("source") {
                // export { a, b as c } from './x'  /  export * from './x'
                let module = string_value(src, ctx.src);
                let mut any = false;
                for ch in named_children(node) {
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
                            mark_exported(ctx, &n, true);
                        }
                        "export_clause" => {
                            for spec in named_children(ch) {
                                if let Some(n) = spec.child_by_field_name("name") {
                                    let name = ctx.text(n).to_string();
                                    mark_exported(ctx, &name, false);
                                }
                            }
                        }
                        "arrow_function" | "function_expression" | "function" | "class" => {
                            visit_function_like(ctx, st, "default", ch, ch, true, true, Vec::new());
                        }
                        _ => walk(ctx, st, ch),
                    }
                }
            }
        }
        "class_declaration" | "abstract_class_declaration" | "class" => visit_class(ctx, st, node, exported, is_default, decorators),
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            let name = node.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_else(|| "default".into());
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
                ctx.add_symbol(s);
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
        "internal_module" | "module" => {
            if let Some(body) = node.child_by_field_name("body") {
                for ch in named_children(body) {
                    visit_top(ctx, st, ch, exported, false, Vec::new());
                }
            }
        }
        "expression_statement" => {
            // module.exports = ... / exports.x = ...
            walk(ctx, st, node);
        }
        "ambient_declaration" => {
            for ch in named_children(node) {
                visit_top(ctx, st, ch, exported, false, Vec::new());
            }
        }
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
    match value.kind() {
        "new_expression" => value.child_by_field_name("constructor").map(|c| ctx.text(c).to_string()),
        "await_expression" => value.named_child(0).and_then(|v| infer_value_type(ctx, v)),
        "as_expression" | "satisfies_expression" => value.named_child(1).map(|t| ctx.text(t).to_string()),
        _ => None,
    }
}

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
        let ty = type_annotation_text(ctx, d.child_by_field_name("type"));
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
            // Destructuring: declare names, walk value.
            declare_pattern(ctx, name_node);
            if let Some(v) = value {
                walk(ctx, st, v);
            }
            continue;
        }
        let name = ctx.text(name_node).to_string();
        if let Some(v) = value {
            if is_function_value(v) {
                visit_function_like(ctx, st, &name, v, node, exported, false, Vec::new());
                continue;
            }
            if v.kind() == "class" {
                visit_class(ctx, st, v, exported, false, Vec::new());
                continue;
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
        if let Some(v) = value {
            ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
            walk(ctx, st, v);
            ctx.pop_scope();
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

fn parse_params(ctx: &Ctx, node: Option<Node>) -> (ParamInfo, String, Vec<(String, Option<String>)>, Vec<(String, Option<String>)>) {
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
        let (pattern, ty, optional, variadic, is_prop) = match p.kind() {
            "required_parameter" | "optional_parameter" => {
                let pat = p.child_by_field_name("pattern");
                let ty = type_annotation_text(ctx, p.child_by_field_name("type"));
                let has_default = p.child_by_field_name("value").is_some();
                let is_prop = children(p).iter().any(|c| matches!(c.kind(), "accessibility_modifier" | "readonly" | "override_modifier"));
                let variadic = pat.map(|x| x.kind() == "rest_pattern").unwrap_or(false);
                (pat, ty, p.kind() == "optional_parameter" || has_default, variadic, is_prop)
            }
            "identifier" => (Some(p), None, false, false, false),
            "assignment_pattern" => (p.child_by_field_name("left"), None, true, false, false),
            "rest_pattern" => (Some(p), None, false, true, false),
            "object_pattern" | "array_pattern" => (Some(p), None, false, false, false),
            _ => continue,
        };
        let Some(pattern) = pattern else { continue };
        let name = ctx.text(pattern).trim_start_matches("...").to_string();
        if name == "this" {
            continue;
        }
        if pattern.kind() == "identifier" || pattern.kind() == "rest_pattern" {
            locals.push((name.clone(), ty.clone()));
            if is_prop {
                props.push((name.clone(), ty.clone()));
            }
        } else {
            let mut tmp = Vec::new();
            collect_pattern_names(ctx, pattern, &mut tmp);
            for n in tmp {
                locals.push((n, None));
            }
        }
        list.push(Param { name: collapse_ws(&name), ty, optional, variadic });
    }
    let inner = strip_parens(ctx.text(node));
    (ParamInfo::from_list(list), collapse_ws(inner), locals, props)
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
    let ret = type_annotation_text(ctx, func.child_by_field_name("return_type"));
    let is_async = children(func).iter().any(|c| c.kind() == "async");
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
    s.type_ref = ret;
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
    }
    let line = line_of(range);
    let id = ctx.add_symbol(s);
    if let Some((m, p)) = route {
        ctx.push_route(&id, &m, &p, "nextjs", line);
    }
    let mut scope =
        Scope { symbol_id: id, kind: Some(SymbolKind::Function), qname: ctx.child_qname(name), is_function: true, ..Default::default() };
    for (n, t) in locals {
        scope.locals.insert(n, t);
    }
    ctx.push_scope(scope);
    if let Some(p) = params_node {
        walk_param_defaults(ctx, st, p);
    }
    if let Some(b) = func.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    ctx.pop_scope();
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

/// Route path for Next.js `app/**/route.ts` and `pages/api/**` files.
fn nextjs_route(path: &str) -> Option<String> {
    let p = path.replace('\\', "/");
    if let Some(idx) = p.find("app/") {
        let rest = &p[idx + 4..];
        let file = rest.rsplit('/').next()?;
        if file.starts_with("route.") {
            let dir = rest.trim_end_matches(file).trim_end_matches('/');
            let segs: Vec<&str> = dir.split('/').filter(|s| !s.is_empty() && !(s.starts_with('(') && s.ends_with(')'))).collect();
            return Some(format!("/{}", segs.join("/")));
        }
    }
    if let Some(idx) = p.find("pages/api/") {
        let rest = &p[idx + 6..];
        let no_ext = rest.rsplit_once('.').map(|(a, _)| a).unwrap_or(rest);
        let no_index = no_ext.trim_end_matches("/index");
        return Some(format!("/{no_index}"));
    }
    None
}

fn visit_class(ctx: &mut Ctx, st: &mut State, node: Node, exported: bool, is_default: bool, decorators: Vec<String>) {
    let name = node.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_else(|| "default".into());
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
    for ch in children(node) {
        if ch.kind() == "decorator" {
            decorators.push(decorator_text(ctx, ch));
            literal_strings(ctx, ch);
        }
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
    s.bases = bases;
    s.docstring = doc_comment(node, ctx.src);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = false;
    let controller = decorators.iter().find(|d| dec_name(d) == "Controller");
    let route_prefix = controller.map(|d| dec_first_string(d).unwrap_or_default());
    let catch_dec = decorators.iter().find(|d| dec_name(d) == "Catch").cloned();
    let module_dec = decorators.iter().find(|d| dec_name(d) == "Module").cloned();
    let injectable_scope = decorators.iter().find(|d| dec_name(d) == "Injectable").map(|d| {
        if d.contains("Scope.REQUEST") {
            "request"
        } else if d.contains("Scope.TRANSIENT") {
            "transient"
        } else {
            "singleton"
        }
    });
    s.attrs = decorators;
    if is_default {
        s.attrs.push("export default".into());
    }
    let line = line_of(node);
    let id = ctx.add_symbol(s);
    let _ = injectable_scope;

    // Field types: class fields + constructor parameter properties.
    let mut fields: HashMap<String, Option<String>> = HashMap::new();
    let body = node.child_by_field_name("body");
    if let Some(body) = body {
        for m in named_children(body) {
            match m.kind() {
                "public_field_definition" | "field_definition" => {
                    let n = m.child_by_field_name("name").or_else(|| m.child_by_field_name("property"));
                    if let Some(n) = n {
                        let ty = type_annotation_text(ctx, m.child_by_field_name("type"))
                            .or_else(|| m.child_by_field_name("value").and_then(|v| infer_value_type(ctx, v)));
                        fields.insert(ctx.text(n).trim_start_matches('#').to_string(), ty);
                    }
                }
                "method_definition" => {
                    let mname = m.child_by_field_name("name").map(|n| ctx.text(n)).unwrap_or("");
                    if mname == "constructor" {
                        let (_, _, _, props) = parse_params(ctx, m.child_by_field_name("parameters"));
                        for (n, t) in props {
                            fields.insert(n, t);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope { symbol_id: id.clone(), kind: Some(SymbolKind::Class), qname, fields, ..Default::default() });
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
    let info = ClassInfo { name: name.clone(), route_prefix: route_prefix.clone(), is_controller: route_prefix.is_some() };
    if let Some(body) = body {
        let mut pending: Vec<String> = Vec::new();
        for m in named_children(body) {
            match m.kind() {
                "decorator" => {
                    pending.push(decorator_text(ctx, m));
                    literal_strings(ctx, m);
                }
                "method_definition" | "method_signature" | "abstract_method_signature" => {
                    let decs = std::mem::take(&mut pending);
                    visit_method(ctx, st, m, &info, decs);
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

fn member_name(ctx: &Ctx, m: Node) -> String {
    m.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default()
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
    let is_abstract = m.kind() != "method_definition" || toks.iter().any(|t| t == "abstract");
    let kind = if name == "constructor" { SymbolKind::Constructor } else { SymbolKind::Method };
    let params_node = m.child_by_field_name("parameters");
    let (params, ptext, locals, _) = parse_params(ctx, params_node);
    let ret = type_annotation_text(ctx, m.child_by_field_name("return_type"));
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
    s.type_ref = ret;
    s.params = Some(params);
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
    let mut scope = Scope { symbol_id: id, kind: Some(kind), qname: ctx.child_qname(&name), is_function: true, ..Default::default() };
    for (n, t) in locals {
        scope.locals.insert(n, t);
    }
    ctx.push_scope(scope);
    let _ = &cls.name;
    if let Some(p) = params_node {
        walk_param_defaults(ctx, st, p);
    }
    if let Some(b) = m.child_by_field_name("body") {
        walk(ctx, st, b);
    }
    ctx.pop_scope();
}

fn visit_field(ctx: &mut Ctx, st: &mut State, m: Node, decorators: Vec<String>) {
    let Some(n) = m.child_by_field_name("name").or_else(|| m.child_by_field_name("property")) else { return };
    let name = ctx.text(n).to_string();
    let toks: Vec<String> =
        children(m).iter().filter(|c| !c.is_named() || c.kind() == "accessibility_modifier").map(|c| ctx.text(*c).to_string()).collect();
    let access = toks.iter().find(|t| matches!(t.as_str(), "private" | "protected" | "public")).cloned();
    let is_static = toks.iter().any(|t| t == "static");
    let ty = type_annotation_text(ctx, m.child_by_field_name("type"));
    let value = m.child_by_field_name("value");
    if let Some(v) = value {
        if is_function_value(v) {
            // Arrow-function class properties behave like methods.
            let id_name = name.clone();
            visit_function_like(ctx, st, &id_name, v, m, true, false, decorators);
            if let Some(last) = ctx.facts.symbols.last_mut() {
                last.kind = SymbolKind::Method;
                if name.starts_with('#') || access.as_deref() == Some("private") {
                    last.visibility = "private".into();
                }
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
    if let Some(v) = value {
        ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
        walk(ctx, st, v);
        ctx.pop_scope();
    }
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
    ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Interface), qname: ctx.child_qname(&name), ..Default::default() });
    if let Some(body) = node.child_by_field_name("body") {
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
                }
                "property_signature" => {
                    let ty = type_annotation_text(ctx, m.child_by_field_name("type"));
                    let mut ps = NewSymbol::new(SymbolKind::Property, &mname, ctx.child_qname(&mname), line_of(m), end_line_of(m));
                    ps.signature = collapse_ws(ctx.text(m)).trim_end_matches([';', ',']).to_string();
                    ps.type_ref = ty;
                    ps.body_hash = token_hash(m, ctx.src);
                    ctx.add_symbol(ps);
                }
                _ => {}
            }
        }
    }
    ctx.pop_scope();
}

// ---------------------------------------------------------------------------
// Statement / expression walker
// ---------------------------------------------------------------------------

fn walk(ctx: &mut Ctx, st: &mut State, node: Node) {
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
                    if n.kind() == "identifier" {
                        let name = ctx.text(n).to_string();
                        let ty = type_annotation_text(ctx, d.child_by_field_name("type"))
                            .or_else(|| value.and_then(|v| infer_value_type(ctx, v)));
                        ctx.declare_local(&name, ty);
                    } else {
                        declare_pattern(ctx, n);
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
                visit_class(ctx, st, node, false, false, Vec::new());
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
            let ty = handler.map(|h| catch_type(ctx, h)).into_iter().collect::<Vec<_>>();
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
                }
            }
            for ch in named_children(node) {
                if Some(ch) != node.child_by_field_name("name") {
                    walk(ctx, st, ch);
                }
            }
        }
        "jsx_attribute" => {
            for ch in named_children(node).into_iter().skip(1) {
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
                declare_pattern(ctx, l);
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
        "decorator" => {}
        _ => walk_children(ctx, st, node),
    }
}

fn walk_children(ctx: &mut Ctx, st: &mut State, node: Node) {
    for ch in named_children(node) {
        walk(ctx, st, ch);
    }
}

fn inline_function(ctx: &mut Ctx, st: &mut State, node: Node) {
    let params_node = node.child_by_field_name("parameters").or_else(|| node.child_by_field_name("parameter"));
    let (_, _, locals, _) = parse_params(ctx, params_node);
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

fn catch_type(ctx: &Ctx, handler: Node) -> String {
    // TS/JS catch params are untyped; `catch (e: SomeError)` is rare. Treat as catch-all.
    if let Some(p) = handler.child_by_field_name("type") {
        return clean_type(ctx.text(p));
    }
    "*".into()
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
        ctx.push_ref(RefKind::Read, prop.trim_start_matches('#'), Some("this".into()), cls, None, line_of(node), None);
        return;
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
                (Some(o), Some(p)) if o.kind() == "this" => ctx.field_type(p.trim_start_matches('#')),
                _ => None,
            }
        }
        "new_expression" => r.child_by_field_name("constructor").map(|c| ctx.text(c).to_string()),
        _ => None,
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
            let prop = prop.trim_start_matches('#').to_string();
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
    let Some(func) = node.child_by_field_name("function") else {
        walk_children(ctx, st, node);
        return;
    };
    let args = node.child_by_field_name("arguments");
    let argc = args.map(count_args);
    let line = line_of(node);
    let type_args = node.child_by_field_name("type_arguments").map(|t| ctx.text(t).to_string());
    let (name, recv_node) = match func.kind() {
        "identifier" => (ctx.text(func).to_string(), None),
        "member_expression" => {
            (func.child_by_field_name("property").map(|p| ctx.text(p).to_string()).unwrap_or_default(), func.child_by_field_name("object"))
        }
        "import" => {
            // dynamic import("./x")
            if let Some((m, _)) = args.and_then(|a| first_string_arg(a, ctx.src)) {
                ctx.facts.imports.push(Import { module: m, imported: None, local: None, kind: ImportKind::Namespace, line });
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
        let base = if receiver.is_none() { name.as_str() } else { receiver.as_deref().unwrap() };
        if TEST_FNS.contains(&base) && arg_list.len() >= 2 {
            let label = string_like(arg_list[0], ctx.src);
            let cb = arg_list.iter().rev().find(|a| is_function_value(**a)).copied();
            if let (Some(label), Some(cb)) = (label, cb) {
                let mut s = NewSymbol::new(SymbolKind::Test, &label, ctx.child_qname(&truncate(&label, 60)), line, end_line_of(node));
                s.signature = format!("{base}(\"{}\")", truncate(&label, 80));
                s.is_test = true;
                s.complexity = cyclomatic(cb, ctx.src, ctx.lang);
                s.body_hash = token_hash(node, ctx.src);
                let id = ctx.add_symbol(s);
                ctx.push_literal(&label, "string", arg_list[0]);
                ctx.push_scope(Scope {
                    symbol_id: id,
                    kind: Some(SymbolKind::Test),
                    qname: ctx.child_qname(&truncate(&label, 60)),
                    is_function: true,
                    ..Default::default()
                });
                let (_, _, locals, _) = parse_params(ctx, cb.child_by_field_name("parameters"));
                for (n, t) in locals {
                    ctx.declare_local(&n, t);
                }
                if let Some(b) = cb.child_by_field_name("body") {
                    walk(ctx, st, b);
                }
                ctx.pop_scope();
                return;
            }
        }
    }

    let recv_lower = receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let recv_last = last_segment(&recv_lower).trim_start_matches('_').trim_start_matches('$').to_string();
    let first_str = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
    let mut handled_args: Vec<Node> = Vec::new();

    // ---- Server routes (express-like) ----
    let lname = name.to_ascii_lowercase();
    let is_route_verb = HTTP_VERBS.contains(&lname.as_str()) || lname == "all";
    let routerish = recv_last == "app"
        || recv_last == "router"
        || recv_last == "server"
        || recv_last.ends_with("router")
        || recv_last == "routes"
        || recv_last == "api" && st.server_file;
    let mut is_server_route = false;
    if st.server_file && is_route_verb && routerish && arg_list.len() >= 2 {
        if let Some(path) = first_str.clone().filter(|p| p.starts_with('/') || p == "*") {
            is_server_route = true;
            let method = if lname == "all" { "ANY".to_string() } else { lname.to_ascii_uppercase() };
            let handler = *arg_list.last().unwrap();
            if is_function_value(handler) {
                let label = format!("{method} {path}");
                let mut s = NewSymbol::new(SymbolKind::Function, &label, ctx.child_qname(&label), line_of(handler), end_line_of(handler));
                s.signature = format!("{label} handler");
                s.is_entrypoint = true;
                s.entry_kind = Some(format!("route {method} {path}"));
                s.complexity = cyclomatic(handler, ctx.src, ctx.lang);
                s.body_hash = token_hash(handler, ctx.src);
                s.is_async = children(handler).iter().any(|c| c.kind() == "async");
                let id = ctx.add_symbol(s);
                ctx.push_route(&id, &method, &path, "express", line);
                ctx.push_scope(Scope {
                    symbol_id: id,
                    kind: Some(SymbolKind::Function),
                    qname: ctx.child_qname(&label),
                    is_function: true,
                    ..Default::default()
                });
                let (_, _, locals, _) =
                    parse_params(ctx, handler.child_by_field_name("parameters").or_else(|| handler.child_by_field_name("parameter")));
                for (n, t) in locals {
                    ctx.declare_local(&n, t);
                }
                if let Some(b) = handler.child_by_field_name("body") {
                    walk(ctx, st, b);
                }
                ctx.pop_scope();
                handled_args.push(handler);
            } else {
                let target = last_segment(ctx.text(handler)).to_string();
                let enclosing = ctx.enclosing();
                ctx.push_route(&format!("ref:{target}|{enclosing}"), &method, &path, "express", line);
            }
        }
    }

    // ---- HTTP client calls ----
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

    let detail = type_args.map(|t| collapse_ws(&t));
    ctx.push_ref(RefKind::Call, &name, receiver, rtype, argc, line, detail);
    if let Some(r) = recv_node {
        walk(ctx, st, r);
    }
    for a in arg_list {
        if !handled_args.contains(&a) {
            walk(ctx, st, a);
        }
    }
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
