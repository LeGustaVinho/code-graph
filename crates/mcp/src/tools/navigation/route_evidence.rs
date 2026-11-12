//! Source excerpts for an indexed registration and its directly resolved calls.

use std::collections::HashMap;
use std::path::Path;

use codegraph_core::extract::{end_line_of, line_of, string_like, truncate};
use codegraph_core::routes::normalize_route;
use codegraph_core::{lang, Language, RefKind};
use codegraph_graph::store::StoredFacts;
use codegraph_graph::{query, EdgeRow, Node};
use codegraph_semantic::Confidence;
use rusqlite::OptionalExtension;
use tree_sitter::Node as SyntaxNode;

use super::{display, node_line, nodes_map, ApiRoute, Ctx, Out, ToolError};

pub(super) fn render(ctx: &Ctx, out: &mut Out, route: Option<&ApiRoute>, handler: &Node) -> Result<(), ToolError> {
    let calls = query::edges_out(ctx.conn, &handler.sym.id, &["CALLS", "RESOLVES_DI"])?;
    let targets = nodes_map(ctx.conn, calls.iter().filter_map(|e| e.target_id.clone()))?;
    let mut wrapper = None;
    if let Some(route) = route {
        out.item(format!("ROUTE {} {} [{}] {}:{}", route.method, route.path, route.framework, route.file, route.line));
        if handler.file_path != route.file || handler.sym.start_line != route.line {
            out.item(format!("  handler {}", node_line(handler)));
        }
        if let Some(src) = read_source(ctx, &route.file)? {
            wrapper = render_registration(ctx, out, route, &src, &calls, &targets)?;
        } else {
            out.item("  registration source unavailable at indexed coordinates");
        }
    } else {
        out.item(format!("HANDLER {}", node_line(handler)));
    }

    let base_file = route.map(|r| r.file.as_str()).unwrap_or(&handler.file_path);
    let mut shown = 0;
    let mut hidden = 0;
    for edge in calls {
        if wrapper.as_deref().is_some_and(|id| edge.target_id.as_deref() == Some(id))
            && route.is_some_and(|r| r.file == edge.source_file && r.line == edge.line)
        {
            continue;
        }
        if shown == 16 {
            hidden += 1;
            continue;
        }
        let file = if edge.source_file == base_file { "" } else { &edge.source_file };
        let line = edge.line;
        let conf = edge.confidence.as_str();
        let target = match edge.target_id.as_ref().and_then(|id| targets.get(id)) {
            Some(n) => format!("{} {}", display(n), n.coords()),
            None => edge.unresolved_symbol.unwrap_or_default(),
        };
        out.item(format!("  call {file}:{line} → {target} [{conf}]"));
        shown += 1;
    }
    if hidden > 0 {
        out.item(format!("  {hidden} more direct call(s)"));
    }
    Ok(())
}

fn read_source(ctx: &Ctx, file: &str) -> Result<Option<String>, ToolError> {
    // Source is optional. Only quote readable project files that still agree with the index.
    let root = &ctx.engine.root;
    let (Ok(abs), Ok(canonical_root)) = (root.join(file).canonicalize(), root.canonicalize()) else { return Ok(None) };
    if !abs.starts_with(canonical_root)
        || !abs.metadata().is_ok_and(|m| m.is_file() && m.len() <= codegraph_core::walk::MAX_FILE_BYTES)
    {
        return Ok(None);
    }
    let hash: Option<String> = ctx.conn.query_row("SELECT content_hash FROM file_meta WHERE file_path = ?1", [file], |r| r.get(0)).optional()?;
    let (Some(hash), Ok(source)) = (hash, std::fs::read_to_string(abs)) else { return Ok(None) };
    Ok((codegraph_core::hash::content_hash(source.as_bytes()) == hash).then_some(source))
}

fn render_registration(
    ctx: &Ctx,
    out: &mut Out,
    route: &ApiRoute,
    src: &str,
    edges: &[EdgeRow],
    targets: &HashMap<String, Node>,
) -> Result<Option<String>, ToolError> {
    let parsed = Language::from_path(Path::new(&route.file)).filter(|l| l.family() == "js").and_then(|l| lang::parse(l, src));
    let registration = match parsed.as_ref() {
        Some(tree) => {
            let wrappers = indexed_wrappers(ctx, route)?;
            registration_call(tree.root_node(), route, src, &wrappers).map(|n| (n, tree.root_node()))
        }
        None => None,
    };
    let Some((call, root)) = registration else {
        if let Some(line) = route.line.checked_sub(1).and_then(|l| src.lines().nth(l as usize)) {
            excerpt(out, "indexed source", route.line, route.line, line, 260);
        }
        return Ok(None);
    };
    excerpt_node(out, "register", call, src, 320);
    if route.framework == "fastify" && matches!(route.method.as_str(), "POST" | "PUT" | "PATCH") {
        render_preceding_parsers(out, call, src);
    }
    let Some(callee) = call.child_by_field_name("function").filter(|n| n.kind() == "identifier") else { return Ok(None) };
    let args = arguments(call);
    let wrapper = unique(edges.iter().filter_map(|edge| {
        if edge.relation != "CALLS" || edge.line != route.line || edge.source_file != route.file || edge.confidence != Confidence::Exact {
            return None;
        }
        let argc = edge.metadata.as_ref()?.get("args")?.as_u64()?;
        let n = targets.get(edge.target_id.as_ref()?)?;
        (argc == args.len() as u64 && n.file_path == route.file && n.sym.name == text(callee, src)).then_some(n)
    }));
    let Some(wrapper) = wrapper else { return Ok(None) };
    out.item(format!("  wrapper :{}-{} {} [exact]", wrapper.sym.start_line, wrapper.sym.end_line, display(wrapper)));
    if route.framework == "fastify" {
        if let Some(options) = fastify_dispatch(root, wrapper, route, &args, src) {
            let dispatch = options.parent().and_then(|args| args.parent()).and_then(|c| c.child_by_field_name("function"));
            if let Some(dispatch) = dispatch {
                excerpt_node(out, "dispatch", dispatch, src, 100);
            }
            if let Some((schema, binding)) = property(options, "schema", src).and_then(|s| schema_body(s, src)) {
                excerpt_node(out, "body wiring", schema, src, 220);
                if let Some(body_arg) = wrapper_arg(wrapper, &args, binding) {
                    let definition = local_const(call, body_arg, src).unwrap_or(body_arg);
                    excerpt_node(out, "body input", definition, src, 320);
                }
            }
            if let Some(callback) = property(options, "handler", src).and_then(|n| n.child_by_field_name("body")) {
                render_callback(ctx, out, callback, wrapper, src)?;
            }
        }
    }
    Ok(Some(wrapper.sym.id.clone()))
}

fn render_preceding_parsers(out: &mut Out, call: SyntaxNode<'_>, src: &str) {
    let Some((receiver, _)) = direct_member_call(call, src) else { return };
    let Some(statement) = call.parent().filter(|n| n.kind() == "expression_statement") else { return };
    let Some(scope) = statement.parent().filter(|n| matches!(n.kind(), "statement_block" | "program")) else { return };
    if !src[scope.start_byte()..statement.start_byte()].contains("ContentTypeParser") {
        return;
    }

    // Only quote direct siblings of the registration, stopping at a possible
    // change to the receiver binding. These excerpts describe source order.
    let mut parsers = Vec::new();
    let mut previous = statement.prev_named_sibling();
    while let Some(statement) = previous {
        if may_rebind(statement, receiver, src) {
            break;
        }
        previous = statement.prev_named_sibling();
        if statement.kind() != "expression_statement" {
            continue;
        }
        let Some(parser) = statement.named_child(0).filter(|n| !n.has_error()) else { continue };
        if direct_member_call(parser, src).is_some_and(|(object, method)| {
            object == receiver && matches!(method, "addContentTypeParser" | "removeContentTypeParser" | "removeAllContentTypeParsers")
        }) {
            parsers.push(parser);
            if parsers.len() == 3 {
                break;
            }
        }
    }
    for parser in parsers.into_iter().rev() {
        excerpt_node(out, "preceding parser", parser, src, 260);
    }
}

fn direct_member_call<'s>(call: SyntaxNode<'_>, src: &'s str) -> Option<(&'s str, &'s str)> {
    if call.kind() != "call_expression" || call.child_by_field_name("optional_chain").is_some() {
        return None;
    }
    let callee = call.child_by_field_name("function").filter(|n| n.kind() == "member_expression")?;
    if callee.child_by_field_name("optional_chain").is_some() {
        return None;
    }
    let object = callee.child_by_field_name("object").filter(|n| n.kind() == "identifier")?;
    Some((text(object, src), text(callee.child_by_field_name("property")?, src)))
}

fn may_rebind(statement: SyntaxNode<'_>, receiver: &str, src: &str) -> bool {
    descendants(statement, None).into_iter().any(|n| {
        let target = match n.kind() {
            "variable_declarator" | "function_declaration" | "class_declaration" => n.child_by_field_name("name"),
            "assignment_expression" | "augmented_assignment_expression" | "for_in_statement" => n.child_by_field_name("left"),
            "update_expression" => n.child_by_field_name("argument"),
            _ => None,
        };
        target.is_some_and(|target| {
            descendants(target, None)
                .into_iter()
                .any(|node| matches!(node.kind(), "identifier" | "shorthand_property_identifier_pattern") && text(node, src) == receiver)
        })
    })
}

fn indexed_wrappers(ctx: &Ctx, route: &ApiRoute) -> Result<Vec<String>, ToolError> {
    let facts: Option<String> =
        ctx.conn.query_row("SELECT facts FROM file_meta WHERE file_path = ?1", [&route.file], |r| r.get(0)).optional()?.flatten();
    let facts = facts.as_deref().and_then(|s| serde_json::from_str::<StoredFacts>(s).ok());
    Ok(facts
        .into_iter()
        .flat_map(|f| f.refs)
        .filter(|r| {
            r.kind == RefKind::Call
                && r.enclosing == route.handler_id
                && r.line == route.line
                && r.detail.as_deref() == Some("route registered through this helper")
        })
        .map(|r| r.name)
        .collect())
}

fn registration_call<'a>(root: SyntaxNode<'a>, route: &ApiRoute, src: &str, wrappers: &[String]) -> Option<SyntaxNode<'a>> {
    let calls: Vec<_> = descendants(root, Some(route.line))
        .into_iter()
        .filter(|&n| n.kind() == "call_expression" && line_of(n) == route.line && !n.has_error())
        .collect();
    let member_matches = |member: &str| member.eq_ignore_ascii_case(&route.method) || member == "all" && route.method == "ANY";
    // Indexed Fastify paths may include a plugin prefix absent from the literal
    // registration. A line alone cannot choose between competing direct calls.
    let ambiguous_direct = route.framework == "fastify"
        && calls
            .iter()
            .filter(|n| {
                n.child_by_field_name("function")
                    .and_then(|f| f.child_by_field_name("property"))
                    .is_some_and(|p| text(p, src) == "route" || member_matches(text(p, src)))
            })
            .take(2)
            .count()
            > 1;
    unique(calls.into_iter().filter(|&n| {
        let Some(callee) = n.child_by_field_name("function") else { return false };
        let route_matches = |a| string_like(a, src).is_some_and(|p| normalize_route(&p) == route.normalized);
        let args = arguments(n);
        if callee.kind() == "identifier" && wrappers.iter().any(|name| name == text(callee, src)) {
            return args.into_iter().any(route_matches);
        }
        if ambiguous_direct {
            return false;
        }
        let Some(member) = callee.child_by_field_name("property").map(|p| text(p, src)) else { return false };
        if member == "route" {
            let [options] = args.as_slice() else { return false };
            return property(*options, "url", src).or_else(|| property(*options, "path", src)).is_some_and(route_matches);
        }
        member_matches(member) && args.first().copied().is_some_and(route_matches)
    }))
}

fn fastify_dispatch<'a>(
    root: SyntaxNode<'a>,
    wrapper: &Node,
    route: &ApiRoute,
    registration_args: &[SyntaxNode<'a>],
    src: &str,
) -> Option<SyntaxNode<'a>> {
    let body = unique(descendants(root, Some(wrapper.sym.start_line)).into_iter().filter_map(|n| {
        if line_of(n) != wrapper.sym.start_line || text(n.child_by_field_name("name")?, src) != wrapper.sym.name {
            return None;
        }
        match n.kind() {
            "variable_declarator" => n.child_by_field_name("value")?.child_by_field_name("body"),
            "function_declaration" => n.child_by_field_name("body"),
            _ => None,
        }
    }))?;
    unique(descendants(body, None).into_iter().filter_map(|n| {
        if n.kind() != "call_expression" || text(n.child_by_field_name("function")?.child_by_field_name("property")?, src) != "route" {
            return None;
        }
        let args = arguments(n);
        let [options] = args.as_slice() else { return None };
        property(*options, "handler", src)?;
        let resolve = |field| {
            let value = property(*options, field, src)?;
            let arg = wrapper_arg(wrapper, registration_args, text(value, src)).unwrap_or(value);
            string_like(arg, src)
        };
        let method = resolve("method")?;
        let path = resolve("url").or_else(|| resolve("path"))?;
        (method.eq_ignore_ascii_case(&route.method) && normalize_route(&path) == route.normalized).then_some(*options)
    }))
}

fn wrapper_arg<'a>(wrapper: &Node, args: &[SyntaxNode<'a>], binding: &str) -> Option<SyntaxNode<'a>> {
    let i = wrapper.sym.params.as_ref()?.list.iter().position(|p| p.name == binding)?;
    args.get(i).copied()
}

fn schema_body<'a, 's>(schema: SyntaxNode<'a>, src: &'s str) -> Option<(SyntaxNode<'a>, &'s str)> {
    if schema.kind() != "object" {
        return None;
    }
    let fields = children(schema);
    let (field, binding) = unique(fields.iter().filter_map(|&item| Some((item, schema_binding(item, src)?))))?;
    if fields.into_iter().any(|item| item != field && may_override(item, "body", src)) {
        return None;
    }
    Some((field, binding))
}

fn schema_binding<'s>(item: SyntaxNode<'_>, src: &'s str) -> Option<&'s str> {
    if item.kind() != "spread_element" {
        return body_value(item, src);
    }
    let mut binding = None;
    for object in spread_objects(item)? {
        match property(object, "body", src) {
            Some(n) if matches!(n.kind(), "identifier" | "shorthand_property_identifier") => {
                let name = text(n, src);
                if binding.is_some_and(|b| b != name) {
                    return None;
                }
                binding = Some(name);
            }
            None if !children(object).into_iter().any(|n| may_override(n, "body", src)) => {}
            _ => return None,
        }
    }
    binding
}

fn spread_objects(item: SyntaxNode<'_>) -> Option<Vec<SyntaxNode<'_>>> {
    let mut value = item.named_child(0)?;
    while value.kind() == "parenthesized_expression" {
        value = value.named_child(0)?;
    }
    let branches = if value.kind() == "ternary_expression" {
        vec![value.child_by_field_name("consequence")?, value.child_by_field_name("alternative")?]
    } else {
        vec![value]
    };
    branches.iter().all(|n| n.kind() == "object").then_some(branches)
}

fn body_value<'s>(pair: SyntaxNode<'_>, src: &'s str) -> Option<&'s str> {
    if pair.kind() == "shorthand_property_identifier" && text(pair, src) == "body" {
        return Some("body");
    }
    let key = pair.child_by_field_name("key")?;
    let value = pair.child_by_field_name("value")?;
    (text(key, src).trim_matches(['\'', '"']) == "body" && value.kind() == "identifier").then(|| text(value, src))
}

fn field_name<'s>(node: SyntaxNode<'_>, src: &'s str) -> Option<&'s str> {
    if node.kind() == "shorthand_property_identifier" {
        return Some(text(node, src));
    }
    let key = node.child_by_field_name("key").or_else(|| node.child_by_field_name("name"))?;
    (key.kind() != "computed_property_name").then(|| text(key, src).trim_matches(['\'', '"']))
}

fn may_override(node: SyntaxNode<'_>, key: &str, src: &str) -> bool {
    if node.kind() == "spread_element" {
        return spread_objects(node)
            .map_or(true, |branches| branches.into_iter().flat_map(children).any(|n| field_name(n, src).map_or(true, |name| name == key)));
    }
    field_name(node, src).map_or(true, |name| name == key)
}

fn local_const<'a>(call: SyntaxNode<'a>, arg: SyntaxNode<'a>, src: &str) -> Option<SyntaxNode<'a>> {
    if arg.kind() != "identifier" {
        return None;
    }
    let start = call.start_byte();
    let statement = call.parent().filter(|s| matches!(s.kind(), "expression_statement" | "return_statement"))?;
    let scope = statement.parent().filter(|s| matches!(s.kind(), "statement_block" | "program"))?;
    unique(children(scope).into_iter().filter(|n| n.kind() == "lexical_declaration").flat_map(children).filter(|&decl| {
        decl.kind() == "variable_declarator"
            && decl.start_byte() < start
            && decl.parent().and_then(|d| d.child(0)).is_some_and(|kw| kw.kind() == "const")
            && decl.child_by_field_name("name").is_some_and(|n| text(n, src) == text(arg, src))
    }))
}

fn render_callback(ctx: &Ctx, out: &mut Out, body: SyntaxNode<'_>, wrapper: &Node, src: &str) -> Result<(), ToolError> {
    let statements: Vec<_> = if body.kind() == "statement_block" {
        children(body)
            .into_iter()
            .filter(|n| matches!(n.kind(), "lexical_declaration" | "return_statement" | "expression_statement"))
            .take(6)
            .collect()
    } else {
        vec![body]
    };
    for &statement in &statements {
        excerpt_node(out, "handler excerpt", statement, src, 200);
    }
    let mut emitted = std::collections::HashSet::new();
    for edge in query::edges_out(ctx.conn, &wrapper.sym.id, &["CALLS"])? {
        if edge.confidence != Confidence::Exact || !statements.iter().any(|&s| (line_of(s)..=end_line_of(s)).contains(&edge.line)) {
            continue;
        }
        let Some(id) = edge.target_id else { continue };
        if emitted.len() == 3 || emitted.contains(&id) {
            continue;
        }
        let Some(target) = query::node(ctx.conn, &id)? else { continue };
        let start = target.sym.start_line;
        let end = target.sym.end_line;
        if target.file_path != wrapper.file_path || start == 0 || end < start || end - start > 7 {
            continue;
        }
        let lines = src.lines().skip((start - 1) as usize).take((end - start + 1) as usize).collect::<Vec<_>>().join("\n");
        excerpt(out, "local helper", start, end, &lines, 400);
        emitted.insert(id);
    }
    Ok(())
}

fn excerpt_node(out: &mut Out, label: &str, n: SyntaxNode<'_>, src: &str, cap: usize) {
    excerpt(out, label, line_of(n), end_line_of(n), text(n, src), cap);
}

fn excerpt(out: &mut Out, label: &str, start: u32, end: u32, src: &str, cap: usize) {
    let range = if end > start { format!("{start}-{end}") } else { start.to_string() };
    out.item(format!("  {label} :{range} | {}", truncate(src.trim(), cap)));
}

fn property<'a>(object: SyntaxNode<'a>, key: &str, src: &str) -> Option<SyntaxNode<'a>> {
    if object.kind() != "object" {
        return None;
    }
    for n in children(object).into_iter().rev() {
        if field_name(n, src) == Some(key) {
            return if n.kind() == "shorthand_property_identifier" { Some(n) } else { n.child_by_field_name("value") };
        }
        if may_override(n, key, src) {
            return None;
        }
    }
    None
}

fn text<'s>(node: SyntaxNode<'_>, src: &'s str) -> &'s str {
    &src[node.byte_range()]
}

fn children(node: SyntaxNode<'_>) -> Vec<SyntaxNode<'_>> {
    node.named_children(&mut node.walk()).filter(|n| n.kind() != "comment").collect()
}

fn arguments(call: SyntaxNode<'_>) -> Vec<SyntaxNode<'_>> {
    call.child_by_field_name("arguments").map(children).unwrap_or_default()
}

fn descendants(root: SyntaxNode<'_>, line: Option<u32>) -> Vec<SyntaxNode<'_>> {
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(node) = stack.pop() {
        if line.is_some_and(|l| !(line_of(node)..=end_line_of(node)).contains(&l)) {
            continue;
        }
        stack.extend(children(node));
        out.push(node);
    }
    out
}

fn unique<T>(mut items: impl Iterator<Item = T>) -> Option<T> {
    let first = items.next()?;
    items.next().is_none().then_some(first)
}
