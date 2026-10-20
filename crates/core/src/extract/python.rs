//! Python extractor (tree-sitter-python).

use tree_sitter::Node;

use super::*;
use crate::routes::join_route;

const ROUTE_ATTRS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options", "route", "api_route", "websocket"];

#[derive(Default)]
struct State {
    /// Router variable → URL prefix (`APIRouter(prefix=...)`, `Blueprint(url_prefix=...)`).
    router_prefixes: HashMap<String, String>,
}

#[derive(Clone, Default)]
struct ClassInfo {
    name: String,
    is_test: bool,
}

fn named_children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}

pub(crate) fn extract(ctx: &mut Ctx, root: Node) {
    let mut st = State::default();
    let stmts = named_children(root);
    // Module docstring.
    if let Some(first) = stmts.first() {
        if let Some(doc) = docstring_of_stmt(ctx, *first) {
            ctx.facts.symbols[0].docstring = Some(doc);
        }
    }
    for s in stmts {
        visit_stmt(ctx, &mut st, s, None);
    }
}

fn docstring_of_stmt(ctx: &Ctx, stmt: Node) -> Option<String> {
    if stmt.kind() != "expression_statement" {
        return None;
    }
    let s = stmt.named_child(0)?;
    if s.kind() != "string" {
        return None;
    }
    let v = string_value(s, ctx.src);
    let first_para = v.trim().split("\n\n").next().unwrap_or("").to_string();
    Some(truncate(&collapse_ws(&first_para), 280))
}

fn visit_stmt(ctx: &mut Ctx, st: &mut State, node: Node, cls: Option<&ClassInfo>) {
    match node.kind() {
        "import_statement" => {
            for ch in named_children(node) {
                let (module, alias) = match ch.kind() {
                    "dotted_name" => (ctx.text(ch).to_string(), None),
                    "aliased_import" => (
                        ch.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default(),
                        ch.child_by_field_name("alias").map(|n| ctx.text(n).to_string()),
                    ),
                    _ => continue,
                };
                let local = alias.unwrap_or_else(|| module.clone());
                ctx.declare_local(&local, None);
                ctx.facts.imports.push(Import {
                    module,
                    imported: None,
                    local: Some(local),
                    kind: ImportKind::Namespace,
                    line: line_of(node),
                });
            }
        }
        "import_from_statement" => {
            let module = node.child_by_field_name("module_name").map(|m| ctx.text(m).to_string()).unwrap_or_default();
            let mut any = false;
            for ch in named_children(node) {
                if Some(ch) == node.child_by_field_name("module_name") {
                    continue;
                }
                let (imported, local) = match ch.kind() {
                    "dotted_name" => {
                        let n = ctx.text(ch).to_string();
                        (n.clone(), n)
                    }
                    "aliased_import" => (
                        ch.child_by_field_name("name").map(|n| ctx.text(n).to_string()).unwrap_or_default(),
                        ch.child_by_field_name("alias").map(|n| ctx.text(n).to_string()).unwrap_or_default(),
                    ),
                    "wildcard_import" => {
                        ctx.facts.imports.push(Import {
                            module: module.clone(),
                            imported: None,
                            local: Some("*".into()),
                            kind: ImportKind::Namespace,
                            line: line_of(node),
                        });
                        any = true;
                        continue;
                    }
                    _ => continue,
                };
                ctx.declare_local(&local, None);
                ctx.facts.imports.push(Import {
                    module: module.clone(),
                    imported: Some(imported),
                    local: Some(local),
                    kind: ImportKind::Named,
                    line: line_of(node),
                });
                any = true;
            }
            if !any {
                ctx.facts.imports.push(Import { module, imported: None, local: None, kind: ImportKind::SideEffect, line: line_of(node) });
            }
        }
        "class_definition" => visit_class(ctx, st, node, Vec::new(), node),
        "function_definition" => visit_function(ctx, st, node, Vec::new(), node, cls),
        "decorated_definition" => {
            let decs: Vec<String> = named_children(node)
                .into_iter()
                .filter(|c| c.kind() == "decorator")
                .map(|d| collapse_ws(ctx.text(d).trim_start_matches('@')))
                .collect();
            if let Some(def) = node.child_by_field_name("definition") {
                match def.kind() {
                    "class_definition" => visit_class(ctx, st, def, decs, node),
                    "function_definition" => visit_function(ctx, st, def, decs, node, cls),
                    _ => {}
                }
            }
        }
        "expression_statement" => {
            let first = node.named_child(0);
            match first {
                Some(a) if a.kind() == "assignment" && !ctx.in_function() => decl_assignment(ctx, st, a, cls.is_some()),
                Some(s) if s.kind() == "string" => {}
                _ => walk(ctx, st, node),
            }
        }
        "if_statement" if !ctx.in_function() => {
            let cond = node.child_by_field_name("condition").map(|c| ctx.text(c).to_string()).unwrap_or_default();
            if cond.contains("__name__") && cond.contains("__main__") {
                let m = &mut ctx.facts.symbols[0];
                m.is_entrypoint = true;
                m.entry_kind = Some("main (__main__ guard)".into());
            }
            // Module-level conditional code: declarations inside still belong to the module.
            for ch in named_children(node) {
                match ch.kind() {
                    "block" => {
                        for s in named_children(ch) {
                            visit_stmt(ctx, st, s, cls);
                        }
                    }
                    "elif_clause" | "else_clause" => {
                        for b in named_children(ch) {
                            if b.kind() == "block" {
                                for s in named_children(b) {
                                    visit_stmt(ctx, st, s, cls);
                                }
                            } else {
                                walk(ctx, st, b);
                            }
                        }
                    }
                    _ => walk(ctx, st, ch),
                }
            }
        }
        "comment" | "pass_statement" => {}
        _ => walk(ctx, st, node),
    }
}

fn infer_call_type(ctx: &Ctx, value: Node) -> Option<String> {
    let v = if value.kind() == "await" { value.named_child(0)? } else { value };
    if v.kind() != "call" {
        return None;
    }
    let f = v.child_by_field_name("function")?;
    let t = ctx.text(f);
    let last = last_segment(t);
    if is_upper_ident(last) {
        Some(last.to_string())
    } else {
        None
    }
}

fn kwarg_string(ctx: &Ctx, args: Node, key: &str) -> Option<String> {
    for a in named_children(args) {
        if a.kind() == "keyword_argument" {
            let n = a.child_by_field_name("name").map(|n| ctx.text(n)).unwrap_or("");
            if n == key {
                return a.child_by_field_name("value").and_then(|v| string_like(v, ctx.src));
            }
        }
    }
    None
}

/// Module-level or class-level assignment: declares a variable / constant / field symbol.
fn decl_assignment(ctx: &mut Ctx, st: &mut State, a: Node, in_class: bool) {
    let left = a.child_by_field_name("left");
    let right = a.child_by_field_name("right");
    let ann = a.child_by_field_name("type").map(|t| ctx.text(t).to_string());
    let Some(left) = left else { return };
    if left.kind() != "identifier" {
        declare_targets(ctx, left);
        if let Some(r) = right {
            walk(ctx, st, r);
        }
        return;
    }
    let name = ctx.text(left).to_string();
    let inferred = ann.clone().or_else(|| right.and_then(|r| infer_call_type(ctx, r)));
    if !in_class {
        ctx.declare_local(&name, inferred.clone());
    }
    // Router prefixes: router = APIRouter(prefix="/items") / bp = Blueprint(..., url_prefix="/x")
    if let Some(r) = right {
        if r.kind() == "call" {
            if let Some(args) = r.child_by_field_name("arguments") {
                if let Some(p) = kwarg_string(ctx, args, "prefix").or_else(|| kwarg_string(ctx, args, "url_prefix")) {
                    st.router_prefixes.insert(name.clone(), p);
                }
            }
        }
    }
    let kind = if in_class {
        SymbolKind::Field
    } else if is_const_name(&name) {
        SymbolKind::Const
    } else {
        SymbolKind::Variable
    };
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(a), end_line_of(a));
    s.signature = match (&ann, right) {
        (Some(t), _) => format!("{name}: {t}"),
        (None, Some(r)) if kind != SymbolKind::Variable => format!("{name} = {}", truncate(&collapse_ws(ctx.text(r)), 80)),
        _ => name.clone(),
    };
    s.type_ref = inferred;
    s.value_text = right.map(|r| ctx.text(r).to_string());
    s.visibility = if name.starts_with('_') && !name.starts_with("__") { "private".into() } else { "public".into() };
    s.is_exported = s.visibility == "public";
    s.is_static = in_class;
    s.body_hash = token_hash(a, ctx.src);
    let id = ctx.add_symbol(s);
    if let Some(r) = right {
        ctx.push_scope(Scope { symbol_id: id, qname: ctx.child_qname(&name), ..Default::default() });
        walk(ctx, st, r);
        ctx.pop_scope();
    }
}

fn declare_targets(ctx: &mut Ctx, n: Node) {
    match n.kind() {
        "identifier" => {
            let name = ctx.text(n).to_string();
            ctx.declare_local(&name, None);
        }
        "pattern_list" | "tuple_pattern" | "list_pattern" | "tuple" | "list" | "list_splat_pattern" => {
            for ch in named_children(n) {
                declare_targets(ctx, ch);
            }
        }
        _ => {}
    }
}

fn visit_class(ctx: &mut Ctx, st: &mut State, node: Node, decorators: Vec<String>, range: Node) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let mut bases = Vec::new();
    let mut attrs = decorators;
    if let Some(sc) = node.child_by_field_name("superclasses") {
        for b in named_children(sc) {
            match b.kind() {
                "keyword_argument" => attrs.push(collapse_ws(ctx.text(b))),
                _ => bases.push(collapse_ws(ctx.text(b))),
            }
        }
    }
    let is_test = name.starts_with("Test") || bases.iter().any(|b| b.ends_with("TestCase"));
    let mut s = NewSymbol::new(SymbolKind::Class, &name, ctx.child_qname(&name), line_of(range), end_line_of(range));
    s.signature = if bases.is_empty() { format!("class {name}") } else { format!("class {name}({})", bases.join(", ")) };
    s.bases = bases.clone();
    s.attrs = attrs;
    s.visibility = if name.starts_with('_') { "private".into() } else { "public".into() };
    s.is_exported = s.visibility == "public";
    s.is_test = is_test;
    s.is_abstract = bases.iter().any(|b| b == "ABC" || b.ends_with("Protocol"));
    let body = node.child_by_field_name("body");
    s.docstring = body.and_then(|b| b.named_child(0)).and_then(|f| docstring_of_stmt(ctx, f));
    s.body_hash = token_hash(node, ctx.src);
    let id = ctx.add_symbol(s);

    // Field types: class-level annotated/assigned attributes + `self.x = ...` in methods.
    let mut fields: HashMap<String, Option<String>> = HashMap::new();
    let mut instance_attrs: Vec<(String, Option<String>, Node)> = Vec::new();
    if let Some(b) = body {
        for stmt in named_children(b) {
            if stmt.kind() == "expression_statement" {
                if let Some(a) = stmt.named_child(0).filter(|a| a.kind() == "assignment") {
                    if let Some(l) = a.child_by_field_name("left").filter(|l| l.kind() == "identifier") {
                        let ty = a
                            .child_by_field_name("type")
                            .map(|t| ctx.text(t).to_string())
                            .or_else(|| a.child_by_field_name("right").and_then(|r| infer_call_type(ctx, r)));
                        fields.insert(ctx.text(l).to_string(), ty);
                    }
                }
            }
        }
        collect_self_attrs(ctx, b, &mut instance_attrs);
        for (n, t, _) in &instance_attrs {
            let e = fields.entry(n.clone()).or_insert(None);
            if e.is_none() {
                *e = t.clone();
            }
        }
    }
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Class), qname, fields: fields.clone(), ..Default::default() });
    decorator_literals(ctx, range);
    let info = ClassInfo { name: name.clone(), is_test };
    let mut declared: Vec<String> = Vec::new();
    if let Some(b) = body {
        for stmt in named_children(b) {
            if stmt.kind() == "expression_statement" {
                if let Some(a) = stmt.named_child(0).filter(|a| a.kind() == "assignment") {
                    if let Some(l) = a.child_by_field_name("left").filter(|l| l.kind() == "identifier") {
                        declared.push(ctx.text(l).to_string());
                    }
                }
            }
        }
        // Instance attributes become field symbols too (first assignment location).
        let mut seen = std::collections::HashSet::new();
        for (n, t, node) in &instance_attrs {
            if declared.contains(n) || !seen.insert(n.clone()) {
                continue;
            }
            let mut fs = NewSymbol::new(SymbolKind::Field, n, ctx.child_qname(n), line_of(*node), end_line_of(*node));
            fs.signature = match t {
                Some(t) => format!("self.{n}: {t}"),
                None => format!("self.{n}"),
            };
            fs.type_ref = t.clone();
            fs.visibility = if n.starts_with('_') && !n.starts_with("__") { "private".into() } else { "public".into() };
            fs.body_hash = token_hash(*node, ctx.src);
            ctx.add_symbol(fs);
        }
        for stmt in named_children(b) {
            visit_stmt(ctx, st, stmt, Some(&info));
        }
    }
    ctx.pop_scope();
}

fn collect_self_attrs<'t>(ctx: &Ctx, node: Node<'t>, out: &mut Vec<(String, Option<String>, Node<'t>)>) {
    // Only look inside methods of this class (not nested classes).
    for ch in named_children(node) {
        match ch.kind() {
            "class_definition" => {}
            "function_definition" | "decorated_definition" => {
                let f = if ch.kind() == "decorated_definition" { ch.child_by_field_name("definition") } else { Some(ch) };
                if let Some(f) = f.filter(|f| f.kind() == "function_definition") {
                    // Parameter types for `self.x = x` inference.
                    let mut ptypes: HashMap<String, String> = HashMap::new();
                    if let Some(params) = f.child_by_field_name("parameters") {
                        for p in named_children(params) {
                            if matches!(p.kind(), "typed_parameter" | "typed_default_parameter") {
                                let pn = p.child_by_field_name("name").or_else(|| p.named_child(0)).map(|x| ctx.text(x).to_string());
                                let pt = p.child_by_field_name("type").map(|x| ctx.text(x).to_string());
                                if let (Some(pn), Some(pt)) = (pn, pt) {
                                    ptypes.insert(pn, pt);
                                }
                            }
                        }
                    }
                    if let Some(body) = f.child_by_field_name("body") {
                        find_self_assignments(ctx, body, &ptypes, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn find_self_assignments<'t>(
    ctx: &Ctx,
    node: Node<'t>,
    ptypes: &HashMap<String, String>,
    out: &mut Vec<(String, Option<String>, Node<'t>)>,
) {
    for ch in named_children(node) {
        match ch.kind() {
            "function_definition" | "class_definition" | "lambda" => {}
            "assignment" | "augmented_assignment" => {
                if let Some(l) = ch.child_by_field_name("left") {
                    if l.kind() == "attribute" {
                        let obj = l.child_by_field_name("object").map(|o| ctx.text(o)).unwrap_or("");
                        if obj == "self" {
                            if let Some(attr) = l.child_by_field_name("attribute") {
                                let name = ctx.text(attr).to_string();
                                let ty = ch.child_by_field_name("type").map(|t| ctx.text(t).to_string()).or_else(|| {
                                    ch.child_by_field_name("right").and_then(|r| {
                                        infer_call_type(ctx, r).or_else(|| {
                                            if r.kind() == "identifier" {
                                                ptypes.get(ctx.text(r)).cloned()
                                            } else {
                                                None
                                            }
                                        })
                                    })
                                });
                                out.push((name, ty, ch));
                            }
                        }
                    }
                }
                find_self_assignments(ctx, ch, ptypes, out);
            }
            _ => find_self_assignments(ctx, ch, ptypes, out),
        }
    }
}

fn parse_params(
    ctx: &Ctx,
    node: Option<Node>,
    drop_first: bool,
) -> (ParamInfo, String, Vec<(String, Option<String>)>, Vec<(String, String)>) {
    // returns (info, text, locals, (param, default-text))
    let Some(node) = node else { return (ParamInfo::default(), String::new(), Vec::new(), Vec::new()) };
    let mut list = Vec::new();
    let mut locals = Vec::new();
    let mut defaults = Vec::new();
    let mut first = true;
    for p in named_children(node) {
        let (name, ty, optional, variadic) = match p.kind() {
            "identifier" => (ctx.text(p).to_string(), None, false, false),
            "typed_parameter" => {
                let inner = p.named_child(0);
                let variadic = inner.map(|i| matches!(i.kind(), "list_splat_pattern" | "dictionary_splat_pattern")).unwrap_or(false);
                let n = inner.map(|i| ctx.text(i).trim_start_matches('*').to_string()).unwrap_or_default();
                (n, p.child_by_field_name("type").map(|t| ctx.text(t).to_string()), false, variadic)
            }
            "default_parameter" | "typed_default_parameter" => {
                let n = p.child_by_field_name("name").map(|x| ctx.text(x).to_string()).unwrap_or_default();
                if let Some(v) = p.child_by_field_name("value") {
                    defaults.push((n.clone(), ctx.text(v).to_string()));
                }
                (n, p.child_by_field_name("type").map(|t| ctx.text(t).to_string()), true, false)
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => (ctx.text(p).trim_start_matches('*').to_string(), None, false, true),
            _ => continue,
        };
        locals.push((name.clone(), ty.clone()));
        if first && drop_first && (name == "self" || name == "cls") {
            first = false;
            continue;
        }
        first = false;
        list.push(Param { name, ty, optional, variadic });
    }
    let inner = strip_parens(ctx.text(node));
    (ParamInfo::from_list(list), collapse_ws(inner), locals, defaults)
}

fn visit_function(ctx: &mut Ctx, st: &mut State, node: Node, decorators: Vec<String>, range: Node, cls: Option<&ClassInfo>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let is_static = decorators.iter().any(|d| d == "staticmethod");
    let (params, ptext, locals, defaults) = parse_params(ctx, node.child_by_field_name("parameters"), cls.is_some() && !is_static);
    let ret = node.child_by_field_name("return_type").map(|t| ctx.text(t).to_string());
    let is_async = {
        let mut c = node.walk();
        let r = node.children(&mut c).any(|ch| ch.kind() == "async");
        r
    };
    let kind = match (cls.is_some(), name.as_str()) {
        (true, "__init__") => SymbolKind::Constructor,
        (true, _) => SymbolKind::Method,
        _ => SymbolKind::Function,
    };
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(range), end_line_of(range));
    let prefix = if is_async { "async def " } else { "def " };
    s.signature = match &ret {
        Some(r) => format!("{prefix}{name}({ptext}) -> {r}"),
        None => format!("{prefix}{name}({ptext})"),
    };
    s.is_async = is_async;
    s.is_static = is_static || decorators.iter().any(|d| d == "classmethod");
    s.visibility =
        if name.starts_with('_') && !(name.starts_with("__") && name.ends_with("__")) { "private".into() } else { "public".into() };
    s.is_exported = s.visibility == "public";
    s.is_abstract = decorators.iter().any(|d| d.contains("abstractmethod"));
    s.type_ref = ret;
    s.params = Some(params);
    let body = node.child_by_field_name("body");
    s.docstring = body.and_then(|b| b.named_child(0)).and_then(|f| docstring_of_stmt(ctx, f));
    s.complexity = cyclomatic(node, ctx.src, ctx.lang);
    s.body_hash = token_hash(range, ctx.src);
    let test_file = is_test_path(ctx.path);
    s.is_test = (name.starts_with("test") && (test_file || cls.map(|c| c.is_test).unwrap_or(false)))
        || decorators.iter().any(|d| d.starts_with("pytest.fixture"));

    let mut route: Option<(String, String, String)> = None;
    let mut handlers: Vec<String> = Vec::new();
    for d in &decorators {
        let head = d.split('(').next().unwrap_or(d);
        let (obj, attr) = match head.rsplit_once('.') {
            Some((o, a)) => (o.to_string(), a.to_string()),
            None => (String::new(), head.to_string()),
        };
        if !obj.is_empty() && ROUTE_ATTRS.contains(&attr.as_str()) {
            let path = d.find('(').and_then(|i| first_quoted(&d[i..])).unwrap_or_default();
            if path.starts_with('/') || path.is_empty() {
                let method = match attr.as_str() {
                    "route" | "api_route" => {
                        if let Some(i) = d.find("methods") {
                            first_quoted(&d[i..]).unwrap_or_else(|| "GET".into()).to_ascii_uppercase()
                        } else {
                            "GET".into()
                        }
                    }
                    "websocket" => "WS".into(),
                    other => other.to_ascii_uppercase(),
                };
                let prefix = st.router_prefixes.get(&obj).cloned().unwrap_or_default();
                let full = join_route(&prefix, &path);
                let fw = if attr == "route" { "flask" } else { "fastapi" };
                route = Some((method, full, fw.to_string()));
            }
        }
        if attr == "exception_handler" || attr == "errorhandler" {
            let inner = d.find('(').map(|i| d[i + 1..].trim_end_matches(')').trim().to_string()).unwrap_or_default();
            handlers.push(if inner.is_empty() { "*".into() } else { last_segment(&inner).to_string() });
        }
        if attr == "command" || head == "click.group" || attr == "callback" && obj.contains("app") {
            s.is_entrypoint = true;
            s.entry_kind = Some("cli command".into());
        }
        if attr == "task" || head == "shared_task" || attr == "on_event" || attr == "subscriber" {
            s.is_entrypoint = true;
            s.entry_kind = Some(format!("event handler @{head}"));
        }
    }
    if let Some((m, p, _)) = &route {
        s.is_entrypoint = true;
        s.entry_kind = Some(format!("route {m} {p}"));
    }
    if !handlers.is_empty() {
        s.is_entrypoint = true;
        s.entry_kind = Some("exception handler".into());
    }
    s.attrs = decorators;
    let line = line_of(range);
    let id = ctx.add_symbol(s);
    if let Some((m, p, fw)) = route {
        ctx.push_route(&id, &m, &p, &fw, line);
    }
    for h in handlers {
        let fw = "python".to_string();
        ctx.facts.global_handlers.push(GlobalHandler { handler: id.clone(), exception: h, framework: fw, line });
    }

    let mut scope = Scope { symbol_id: id, kind: Some(kind), qname: ctx.child_qname(&name), is_function: true, ..Default::default() };
    for (n, t) in locals {
        let t = if (n == "self" || n == "cls") && cls.is_some() { cls.map(|c| c.name.clone()) } else { t };
        scope.locals.insert(n, t);
    }
    ctx.push_scope(scope);
    decorator_literals(ctx, range);
    // FastAPI dependency injection: param=Depends(provider)
    for (pname, default) in &defaults {
        if let Some(inner) = default.strip_prefix("Depends(").and_then(|x| x.strip_suffix(')')) {
            let provider = inner.trim();
            let provider = if provider.is_empty() { pname.as_str() } else { provider };
            ctx.push_di(pname, Some(last_segment(provider)), "request", "fastapi", line);
            if provider.contains("current_user") || provider.contains("auth") {
                let id = ctx.enclosing();
                if let Some(sym) = ctx.symbol_mut(&id) {
                    sym.capabilities.insert("AUTH_REQUIRED".into());
                }
            }
        }
    }
    if let Some(params) = node.child_by_field_name("parameters") {
        for p in named_children(params) {
            if let Some(v) = p.child_by_field_name("value") {
                walk(ctx, st, v);
            }
        }
    }
    if let Some(b) = body {
        let stmts = named_children(b);
        for (i, s) in stmts.into_iter().enumerate() {
            if i == 0 && docstring_of_stmt(ctx, s).is_some() {
                continue;
            }
            walk(ctx, st, s);
        }
    }
    ctx.pop_scope();
}

/// Records string literals used inside decorators (routes, handler names) of a definition.
fn decorator_literals(ctx: &mut Ctx, range: Node) {
    if range.kind() != "decorated_definition" {
        return;
    }
    fn strings(ctx: &mut Ctx, n: Node) {
        if n.kind() == "string" {
            let v = string_value(n, ctx.src);
            ctx.push_literal(&v, "string", n);
            return;
        }
        for ch in named_children(n) {
            strings(ctx, ch);
        }
    }
    for d in named_children(range) {
        if d.kind() == "decorator" {
            strings(ctx, d);
        }
    }
}

// ---------------------------------------------------------------------------
// Statement / expression walker
// ---------------------------------------------------------------------------

fn walk(ctx: &mut Ctx, st: &mut State, node: Node) {
    match node.kind() {
        "call" => call(ctx, st, node),
        "raise_statement" => {
            let Some(expr) = node.named_child(0) else { return };
            match expr.kind() {
                "call" => {
                    let f = expr.child_by_field_name("function");
                    let ty = f.map(|f| last_segment(ctx.text(f)).to_string()).unwrap_or_default();
                    let args = expr.child_by_field_name("arguments");
                    let msg = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
                    ctx.push_ref(RefKind::Throw, &ty, None, None, None, line_of(node), msg);
                    if let Some(a) = args {
                        walk(ctx, st, a);
                    }
                }
                "identifier" | "attribute" => {
                    let t = ctx.text(expr).to_string();
                    if expr.kind() == "identifier" && ctx.is_local(&t) {
                        if let Some(ty) = ctx.var_type(&t) {
                            ctx.push_ref(RefKind::Throw, &clean_type(&ty), None, None, None, line_of(node), None);
                        }
                    } else {
                        ctx.push_ref(RefKind::Throw, last_segment(&t), None, None, None, line_of(node), None);
                    }
                }
                _ => walk(ctx, st, expr),
            }
        }
        "try_statement" => {
            let mut types = Vec::new();
            let mut clauses = Vec::new();
            for ch in named_children(node) {
                if matches!(ch.kind(), "except_clause" | "except_group_clause") {
                    let (tys, alias) = except_types(ctx, ch);
                    types.extend(tys.clone());
                    clauses.push((ch, tys, alias));
                }
            }
            ctx.push_catch_frame(types);
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
            ctx.pop_catch_frame();
            for (clause, tys, alias) in clauses {
                for t in &tys {
                    ctx.push_ref(RefKind::Catch, t, None, None, None, line_of(clause), None);
                }
                if let Some(a) = alias {
                    ctx.declare_local(&a, tys.first().cloned());
                }
                for ch in named_children(clause) {
                    if ch.kind() == "block" {
                        walk(ctx, st, ch);
                    }
                }
            }
            for ch in named_children(node) {
                if matches!(ch.kind(), "else_clause" | "finally_clause") {
                    walk(ctx, st, ch);
                }
            }
        }
        "assignment" | "augmented_assignment" => {
            let right = node.child_by_field_name("right");
            if let Some(l) = node.child_by_field_name("left") {
                let ann = node.child_by_field_name("type").map(|t| ctx.text(t).to_string());
                let ty = ann.or_else(|| right.and_then(|r| infer_call_type(ctx, r)));
                write_target(ctx, st, l, ty, node.kind() == "augmented_assignment");
            }
            if let Some(r) = right {
                walk(ctx, st, r);
            }
        }
        "global_statement" | "nonlocal_statement" => {
            for ch in named_children(node) {
                let n = ctx.text(ch).to_string();
                if let Some(s) = ctx.scopes.last_mut() {
                    s.globals.insert(n);
                }
            }
        }
        "attribute" => {
            let obj = node.child_by_field_name("object");
            let attr = node.child_by_field_name("attribute").map(|a| ctx.text(a).to_string()).unwrap_or_default();
            if let Some(o) = obj {
                let ot = ctx.text(o);
                if ot == "self" {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Read, &attr, Some("self".into()), cls, None, line_of(node), None);
                    return;
                }
                if ot == "settings" || ot.ends_with(".settings") {
                    ctx.push_config(&attr, "settings", line_of(node), None);
                }
                walk(ctx, st, o);
            }
        }
        "subscript" => {
            if let Some(v) = node.child_by_field_name("value") {
                if ctx.text(v) == "os.environ" {
                    if let Some(sub) = node.child_by_field_name("subscript") {
                        if let Some(k) = string_like(sub, ctx.src) {
                            ctx.push_config(&k, "os.environ", line_of(node), None);
                        }
                    }
                }
            }
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
        "identifier" => {
            let name = ctx.text(node).to_string();
            if ctx.is_local(&name)
                || matches!(
                    name.as_str(),
                    "self"
                        | "cls"
                        | "None"
                        | "True"
                        | "False"
                        | "print"
                        | "len"
                        | "str"
                        | "int"
                        | "dict"
                        | "list"
                        | "range"
                        | "isinstance"
                        | "super"
                )
            {
                return;
            }
            ctx.push_ref(RefKind::Read, &name, None, None, None, line_of(node), None);
        }
        "await" => {
            ctx.mark_await();
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
        "lambda" => {
            if let Some(p) = node.child_by_field_name("parameters") {
                let (_, _, locals, _) = parse_params(ctx, Some(p), false);
                for (n, t) in locals {
                    ctx.declare_local(&n, t);
                }
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
        }
        "for_statement" | "for_in_clause" => {
            if let Some(l) = node.child_by_field_name("left") {
                declare_targets(ctx, l);
            }
            for ch in named_children(node) {
                if Some(ch) != node.child_by_field_name("left") {
                    walk(ctx, st, ch);
                }
            }
        }
        "with_item" | "as_pattern" => {
            for ch in named_children(node) {
                if ch.kind() == "as_pattern_target" {
                    for t in named_children(ch) {
                        declare_targets(ctx, t);
                    }
                } else {
                    walk(ctx, st, ch);
                }
            }
        }
        "named_expression" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                ctx.declare_local(&name, None);
            }
            if let Some(v) = node.child_by_field_name("value") {
                walk(ctx, st, v);
            }
        }
        "function_definition" => {
            if ctx.in_function() {
                if let Some(n) = node.child_by_field_name("name") {
                    let name = ctx.text(n).to_string();
                    ctx.declare_local(&name, None);
                }
                let (_, _, locals, _) = parse_params(ctx, node.child_by_field_name("parameters"), false);
                for (n, t) in locals {
                    ctx.declare_local(&n, t);
                }
                if let Some(b) = node.child_by_field_name("body") {
                    walk(ctx, st, b);
                }
            } else {
                visit_function(ctx, st, node, Vec::new(), node, None);
            }
        }
        "decorated_definition" | "class_definition" => {
            if ctx.in_function() {
                if let Some(def) = node.child_by_field_name("definition").or(Some(node)) {
                    if def.kind() == "function_definition" {
                        walk(ctx, st, def);
                    }
                }
            } else {
                visit_stmt(ctx, st, node, None);
            }
        }
        "keyword_argument" => {
            if let Some(v) = node.child_by_field_name("value") {
                walk(ctx, st, v);
            }
        }
        "string" => {
            let is_fstring = has_child_kind(node, "interpolation");
            let v = if is_fstring { template_text(node, ctx.src) } else { string_value(node, ctx.src) };
            ctx.push_literal(&v, if is_fstring { "template" } else { "string" }, node);
            if is_fstring {
                for ch in named_children(node) {
                    if ch.kind() == "interpolation" {
                        walk(ctx, st, ch);
                    }
                }
            }
        }
        "concatenated_string" => {
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
        "integer" | "float" => {
            let v = ctx.text(node).to_string();
            ctx.push_literal(&v, "number", node);
        }
        "type" | "comment" | "decorator" | "import_statement" | "import_from_statement" | "pass_statement" => {}
        _ => {
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
    }
}

fn except_types(ctx: &Ctx, clause: Node) -> (Vec<String>, Option<String>) {
    let mut types = Vec::new();
    let mut alias = None;
    let push_type = |n: Node, types: &mut Vec<String>| match n.kind() {
        "tuple" | "parenthesized_expression" | "expression_list" => {
            for c in named_children(n) {
                types.push(last_segment(ctx.text(c)).to_string());
            }
        }
        _ => types.push(last_segment(ctx.text(n)).to_string()),
    };
    for ch in named_children(clause) {
        match ch.kind() {
            "block" | "comment" => {}
            "as_pattern" => {
                if let Some(t) = ch.named_child(0) {
                    push_type(t, &mut types);
                }
                for c in named_children(ch) {
                    if c.kind() == "as_pattern_target" {
                        alias = Some(ctx.text(c).to_string());
                    }
                }
            }
            "identifier" if !types.is_empty() => alias = Some(ctx.text(ch).to_string()),
            _ => push_type(ch, &mut types),
        }
    }
    if types.is_empty() {
        types.push("*".into());
    }
    (types, alias)
}

fn receiver_info(ctx: &Ctx, r: Node) -> (String, Option<String>) {
    let text = simplify_receiver(ctx.text(r));
    let ty = match r.kind() {
        "identifier" => {
            if text == "self" || text == "cls" {
                ctx.class_name()
            } else {
                ctx.var_type(&text)
            }
        }
        "attribute" => {
            let o = r.child_by_field_name("object").map(|o| ctx.text(o).to_string());
            let a = r.child_by_field_name("attribute").map(|a| ctx.text(a).to_string());
            match (o.as_deref(), a) {
                (Some("self"), Some(a)) => ctx.field_type(&a),
                _ => None,
            }
        }
        "call" => infer_call_type(ctx, r),
        _ => None,
    };
    (text, ty)
}

fn write_target(ctx: &mut Ctx, st: &mut State, left: Node, ty: Option<String>, augmented: bool) {
    let line = line_of(left);
    match left.kind() {
        "identifier" => {
            let name = ctx.text(left).to_string();
            let is_global = ctx.scopes.last().map(|s| s.globals.contains(&name)).unwrap_or(false);
            if is_global {
                ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
                ctx.push_mutation(&name, "global_var", line);
            } else if ctx.in_function() {
                if augmented && !ctx.is_local(&name) {
                    ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
                } else {
                    ctx.declare_local(&name, ty);
                }
            } else {
                ctx.declare_local(&name, ty);
            }
        }
        "attribute" => {
            let obj = left.child_by_field_name("object");
            let attr = left.child_by_field_name("attribute").map(|a| ctx.text(a).to_string()).unwrap_or_default();
            match obj {
                Some(o) if ctx.text(o) == "self" => {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Write, &attr, Some("self".into()), cls.clone(), None, line, None);
                    ctx.push_mutation(&format!("{}.{}", cls.unwrap_or_default(), attr), "local_field", line);
                }
                Some(o) => {
                    let (rtext, rtype) = receiver_info(ctx, o);
                    let owner = rtype.clone().map(|t| clean_type(&t)).unwrap_or_else(|| rtext.clone());
                    ctx.push_ref(RefKind::Write, &attr, Some(rtext), rtype, None, line, None);
                    ctx.push_mutation(&format!("{owner}.{attr}"), "object_field", line);
                    walk(ctx, st, o);
                }
                None => {}
            }
        }
        "subscript" => {
            if let Some(v) = left.child_by_field_name("value") {
                if v.kind() == "identifier" {
                    let n = ctx.text(v).to_string();
                    if !ctx.is_local(&n) && ctx.is_module_var(&n) && ctx.in_function() {
                        ctx.push_ref(RefKind::Write, &n, None, None, None, line, None);
                        ctx.push_mutation(&n, "global_var", line);
                    }
                } else if v.kind() == "attribute" {
                    let o = v.child_by_field_name("object").map(|o| ctx.text(o).to_string());
                    if o.as_deref() == Some("self") {
                        let attr = v.child_by_field_name("attribute").map(|a| ctx.text(a).to_string()).unwrap_or_default();
                        let cls = ctx.class_name();
                        ctx.push_ref(RefKind::Write, &attr, Some("self".into()), cls.clone(), None, line, None);
                        ctx.push_mutation(&format!("{}.{}", cls.unwrap_or_default(), attr), "local_field", line);
                    }
                }
            }
            for ch in named_children(left) {
                walk(ctx, st, ch);
            }
        }
        "pattern_list" | "tuple_pattern" | "list_pattern" => {
            for ch in named_children(left) {
                write_target(ctx, st, ch, None, false);
            }
        }
        _ => walk(ctx, st, left),
    }
}

fn call(ctx: &mut Ctx, st: &mut State, node: Node) {
    let Some(func) = node.child_by_field_name("function") else { return };
    let args = node.child_by_field_name("arguments");
    let argc = args.map(|a| {
        let mut c = a.walk();
        let n = a.named_children(&mut c).filter(|x| !matches!(x.kind(), "comment")).count() as u32;
        n
    });
    let line = line_of(node);
    let (name, recv_node) = match func.kind() {
        "identifier" => (ctx.text(func).to_string(), None),
        "attribute" => {
            (func.child_by_field_name("attribute").map(|a| ctx.text(a).to_string()).unwrap_or_default(), func.child_by_field_name("object"))
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
    let recv_lower = receiver.as_deref().unwrap_or("").to_ascii_lowercase();
    let recv_last = last_segment(&recv_lower).trim_start_matches('_').to_string();
    let first_str = args.and_then(|a| first_string_arg(a, ctx.src)).map(|(s, _)| s);
    let arg_nodes: Vec<Node> = args.map(named_children).unwrap_or_default();

    // ---- Configuration ----
    match (recv_lower.as_str(), name.as_str()) {
        ("os.environ", "get") | ("os", "getenv") | ("environ", "get") | ("os.environ", "setdefault") => {
            if let Some(k) = first_str.clone() {
                let d = arg_nodes.get(1).map(|n| ctx.text(*n).to_string());
                ctx.push_config(&k, "os.environ", line, d);
            }
        }
        ("", "config") | ("", "env") | ("env", "str") | ("env", "int") | ("env", "bool") => {
            if let Some(k) = first_str.clone() {
                let d = args.and_then(|a| {
                    named_children(a).into_iter().find(|x| {
                        x.kind() == "keyword_argument" && x.child_by_field_name("name").map(|n| ctx.text(n) == "default").unwrap_or(false)
                    })
                });
                ctx.push_config(&k, "env", line, d.and_then(|x| x.child_by_field_name("value")).map(|v| ctx.text(v).to_string()));
            }
        }
        _ => {}
    }
    // ---- HTTP clients ----
    if matches!(name.as_str(), "get" | "post" | "put" | "delete" | "patch" | "head" | "request")
        && (matches!(recv_last.as_str(), "requests" | "httpx" | "session" | "client" | "aiohttp" | "http") || recv_last.ends_with("client"))
    {
        let url = if name == "request" { arg_nodes.get(1).and_then(|n| string_like(*n, ctx.src)) } else { first_str.clone() };
        if let Some(url) = url.filter(|u| u.contains('/')) {
            let method = if name == "request" { first_str.clone().unwrap_or_else(|| "GET".into()) } else { name.to_ascii_uppercase() };
            ctx.push_http_call(&method, &url, receiver.as_deref().unwrap_or("requests"), line);
        }
    }
    // ---- Mutations ----
    match name.as_str() {
        "add" | "add_all" | "delete" | "merge" | "bulk_save_objects" | "bulk_insert_mappings"
            if recv_last.contains("session") || recv_last == "db" =>
        {
            let target = arg_nodes
                .first()
                .filter(|n| n.kind() == "identifier")
                .and_then(|n| ctx.var_type(ctx.text(*n)))
                .map(|t| clean_type(&t))
                .unwrap_or_else(|| receiver.clone().unwrap_or_default());
            ctx.push_mutation(&target, "database", line);
        }
        "create" | "update" | "delete" | "bulk_create" | "bulk_update" | "get_or_create" | "update_or_create"
            if recv_lower.ends_with(".objects") || recv_lower.contains(".objects.") =>
        {
            let model = recv_lower.split('.').next().unwrap_or("").to_string();
            let model = receiver.as_deref().and_then(|r| r.split('.').next()).unwrap_or(&model).to_string();
            ctx.push_mutation(&model, "database", line);
        }
        "save" if rtype.as_deref().map(is_upper_ident).unwrap_or(false) => {
            ctx.push_mutation(&clean_type(rtype.as_deref().unwrap()), "database", line);
        }
        "insert_one" | "insert_many" | "update_one" | "update_many" | "delete_one" | "delete_many" | "replace_one"
            if !(recv_lower.contains("cache") || recv_lower.contains("redis")) =>
        {
            ctx.push_mutation(receiver.as_deref().unwrap_or("collection"), "database", line);
        }
        "set" | "delete" | "hset" | "setex" | "incr" | "expire" | "set_many" | "delete_many"
            if recv_lower.contains("cache") || recv_lower.contains("redis") =>
        {
            let target = first_str.clone().map(|k| format!("cache:{k}")).unwrap_or_else(|| receiver.clone().unwrap_or_default());
            ctx.push_mutation(&target, "cache", line);
        }
        "run" if recv_last == "uvicorn" || recv_last == "app" => {
            let enclosing = ctx.enclosing();
            if let Some(s) = ctx.symbol_mut(&enclosing) {
                if !s.is_entrypoint {
                    s.is_entrypoint = true;
                    s.entry_kind = Some("server start".into());
                }
            }
        }
        // Dict-style mutation of module-level state: CACHE.update(...), REGISTRY.append(...)
        "append" | "extend" | "update" | "pop" | "clear" | "insert" | "remove" | "setdefault" | "add" | "discard" => {
            if let Some(r) = recv_node.filter(|r| r.kind() == "identifier") {
                let n = ctx.text(r).to_string();
                if !ctx.is_local(&n) && ctx.is_module_var(&n) && ctx.in_function() {
                    ctx.push_ref(RefKind::Write, &n, None, None, None, line, None);
                    ctx.push_mutation(&n, "global_var", line);
                }
            }
        }
        _ => {}
    }

    ctx.push_ref(RefKind::Call, &name, receiver, rtype, argc, line, None);
    if let Some(r) = recv_node {
        walk(ctx, st, r);
    }
    for a in arg_nodes {
        walk(ctx, st, a);
    }
}
