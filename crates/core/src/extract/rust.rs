//! Rust extractor (tree-sitter-rust).
//!
//! Mapping onto the language-neutral model:
//! * `struct` / `union` → struct, `enum` → enum (variants are constants), `trait` → abstract
//!   interface, `type` → type alias, `macro_rules!` → function.
//! * `impl` blocks are their own symbols ([`SymbolKind::Impl`], named after the implemented
//!   type, implemented trait as base). The semantic index merges them with the type wherever
//!   it is declared, so `x.method()` finds methods of every `impl` of `x`'s type.
//! * `use` / `mod x;` / `extern crate` → imports with Rust paths (`crate::models`); the
//!   semantic index maps them onto files through the crate layout.
//! * Path calls keep the path as receiver with a trailing `::` (`Foo::new()` → receiver
//!   `Foo::`, `crate::db::open()` → `crate::db::`), which tells them apart from method calls.
//! * Macro arguments are token trees for Tree-sitter: they are re-parsed as expressions (see
//!   [`macro_trees`]) so calls inside `assert_eq!(f(x), 1)` or `format!("{}", g())` count.

use std::collections::HashSet;

use tree_sitter::{Node, Tree};

use super::*;

/// Macros of std / ubiquitous crates: never project call targets.
const STD_MACROS: &[&str] = &[
    "println",
    "print",
    "eprintln",
    "eprint",
    "format",
    "format_args",
    "write",
    "writeln",
    "vec",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
    "matches",
    "concat",
    "stringify",
    "include",
    "include_str",
    "include_bytes",
    "env",
    "option_env",
    "cfg",
    "line",
    "column",
    "file",
    "module_path",
    "dbg",
    "compile_error",
    "thread_local",
    "lazy_static",
    "info",
    "warn",
    "error",
    "debug",
    "trace",
    "event",
    "span",
    "info_span",
    "debug_span",
    "error_span",
    "trace_span",
    "warn_span",
    "bail",
    "anyhow",
    "ensure",
    "json",
    "quote",
    "quote_spanned",
    "parse_quote",
    "select",
    "join",
    "try_join",
    "pin",
    "addr_of",
    "addr_of_mut",
    "query",
    "query_as",
    "query_scalar",
];

/// Macros whose arguments are not expressions (token streams, item lists, markup DSLs).
const OPAQUE_MACROS: &[&str] =
    &["quote", "quote_spanned", "parse_quote", "macro_rules", "thread_local", "lazy_static", "html", "rsx", "view", "cfg_if"];

const LOG_MACROS: &[&str] = &["info", "warn", "error", "debug", "trace", "event"];

const ROUTE_ATTRS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options", "trace", "connect", "route"];

const ROUTER_METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options", "trace", "any"];

const TEST_ATTRS: &[&str] = &[
    "test",
    "tokio::test",
    "async_std::test",
    "actix_rt::test",
    "actix_web::test",
    "rstest",
    "bench",
    "wasm_bindgen_test",
    "quickcheck",
    "proptest",
    "test_case",
    "serial",
    "sqlx::test",
];

/// Methods returning (a view of) their receiver: the receiver type carries through.
const IDENTITY_METHODS: &[&str] = &[
    "clone",
    "as_ref",
    "as_mut",
    "borrow",
    "borrow_mut",
    "lock",
    "read",
    "write",
    "to_owned",
    "deref",
    "deref_mut",
    "as_deref",
    "ok",
    "cloned",
];

/// Methods unwrapping an `Option` / `Result`.
const UNWRAP_METHODS: &[&str] = &["unwrap", "expect", "unwrap_or", "unwrap_or_default", "unwrap_or_else", "unwrap_unchecked"];

/// `Type::f()` names that construct a `Type`.
fn is_ctor_name(n: &str) -> bool {
    matches!(n, "new" | "default" | "from" | "open" | "connect" | "create" | "builder" | "try_new" | "try_from")
        || n.starts_with("new_")
        || n.starts_with("with_")
        || n.starts_with("from_")
}

/// Types named so often that `USES_TYPE` facts for them are noise.
const COMMON_TYPES: &[&str] = &[
    "String",
    "str",
    "Vec",
    "Option",
    "Result",
    "Box",
    "Rc",
    "Arc",
    "HashMap",
    "HashSet",
    "BTreeMap",
    "BTreeSet",
    "Self",
    "Mutex",
    "RwLock",
    "RefCell",
    "Cell",
    "Cow",
    "Path",
    "PathBuf",
    "Duration",
    "Instant",
    "Error",
    "VecDeque",
    "Ordering",
    "Default",
    "Debug",
    "Display",
    "Clone",
    "Send",
    "Sync",
    "Sized",
    "Fn",
    "FnMut",
    "FnOnce",
    "Iterator",
    "IntoIterator",
    "Into",
    "From",
    "AsRef",
    "PhantomData",
];

struct State<'t> {
    /// Enclosing inline `mod x { … }` blocks, outermost first.
    mod_path: Vec<String>,
    /// Inside `#[cfg(test)]` code: everything declared there is test code.
    in_test: bool,
    /// Type that `Self` / `self` stand for (enclosing `impl` or `trait`).
    self_type: Option<String>,
    /// Inside `impl Trait for T`: members take the trait's (public) visibility.
    in_trait_impl: bool,
    /// Fields of the structs declared in this file (seed `impl` scopes).
    struct_fields: HashMap<String, HashMap<String, Option<String>>>,
    /// Macro arguments re-parsed as expressions, one tree per nesting level.
    macro_roots: Vec<Node<'t>>,
    /// Level of the tree being walked (0 = the file itself).
    level: usize,
    /// Web framework the file imports (`axum`, `actix`, `rocket`, …).
    web: Option<&'static str>,
    /// (enclosing, type) pairs already recorded as `USES_TYPE` facts.
    type_uses: HashSet<(String, String)>,
}

fn named_children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}

pub(crate) fn extract(ctx: &mut Ctx, root: Node) {
    let trees = macro_trees(ctx.src, root);
    let mut st = State {
        mod_path: Vec::new(),
        in_test: false,
        self_type: None,
        in_trait_impl: false,
        struct_fields: HashMap::new(),
        macro_roots: trees.iter().map(|t| t.root_node()).collect(),
        level: 0,
        web: None,
        type_uses: HashSet::new(),
    };
    collect_struct_fields(ctx, root, &mut st.struct_fields);
    ctx.facts.symbols[0].docstring = inner_doc(ctx, root);
    visit_items(ctx, &mut st, root);
}

// ---------------------------------------------------------------------------
// Macro arguments
// ---------------------------------------------------------------------------

/// Token trees of the expression macros in `root` (not descending into them).
fn macro_arg_ranges(root: Node) -> Vec<tree_sitter::Range> {
    let mut out: Vec<tree_sitter::Range> = Vec::new();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "macro_invocation" {
            let name = n.child_by_field_name("macro").map(|m| m.kind() == "identifier" || m.kind() == "scoped_identifier").unwrap_or(false);
            let item_position = n.parent().map(|p| matches!(p.kind(), "source_file" | "declaration_list")).unwrap_or(true);
            if name && !item_position {
                if let Some(tt) = named_children(n).into_iter().find(|c| c.kind() == "token_tree") {
                    out.push(tt.range());
                }
            }
            continue;
        }
        if n.kind() == "macro_definition" || n.kind() == "token_tree" {
            continue;
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
    out.sort_unstable_by_key(|r| r.start_byte);
    out
}

/// Re-parses macro arguments as expressions: only the token trees of one nesting level are
/// parsed (Tree-sitter included ranges), so node positions, line numbers and `ctx.text` stay
/// those of the real file. The token trees parse as tuples / arrays / blocks; nested macros
/// produce the next level.
fn macro_trees(src: &str, root: Node) -> Vec<Tree> {
    let mut out: Vec<Tree> = Vec::new();
    let mut tts = macro_arg_ranges(root);
    let mut buf = src.as_bytes().to_vec();
    while !tts.is_empty() && out.len() < 3 {
        // A `;` right after every argument list (written over the byte that follows it, in a
        // copy) keeps neighbours from parsing as one expression: `(a)(b)` would be a call.
        let mut ranges = Vec::with_capacity(tts.len() * 2);
        for tt in &tts {
            ranges.push(*tt);
            let e = tt.end_byte;
            if e < buf.len() && tts.iter().all(|o| o.start_byte != e) {
                buf[e] = b';';
                let p = tt.end_point;
                let end = tree_sitter::Point { row: p.row, column: p.column + 1 };
                ranges.push(tree_sitter::Range { start_byte: e, end_byte: e + 1, start_point: p, end_point: end });
            }
        }
        let tree = crate::lang::parse_ranges(Language::Rust, &buf, &ranges);
        for tt in &tts {
            if tt.end_byte < buf.len() {
                buf[tt.end_byte] = src.as_bytes()[tt.end_byte];
            }
        }
        let Some(tree) = tree else { break };
        tts = macro_arg_ranges(tree.root_node());
        out.push(tree);
    }
    out
}

/// Parsed arguments of the macro token tree `[s, e)` in a re-parsed level.
fn macro_args(root: Node, s: usize, e: usize) -> Vec<Node> {
    let Some(mut n) = root.descendant_for_byte_range(s, e) else { return Vec::new() };
    while n.start_byte() > s || n.end_byte() < e {
        match n.parent() {
            Some(p) => n = p,
            None => break,
        }
    }
    if n.start_byte() == s && n.end_byte() == e {
        return vec![n];
    }
    named_children(n).into_iter().filter(|c| c.start_byte() >= s && c.end_byte() <= e).collect()
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Path without generic arguments or whitespace: `Vec::<T>` → `Vec`, `<T as Repo>` → `Repo`.
pub fn clean_path(t: &str) -> String {
    let mut t = t.trim().to_string();
    if t.starts_with('<') {
        // Qualified self type `<T as Trait>::…` names the trait.
        let mut depth = 0;
        let mut end = t.len();
        for (i, c) in t.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let inner = &t[1..end.min(t.len())];
        let head = match inner.split_once(" as ") {
            Some((_, tr)) => tr.trim().to_string(),
            None => inner.trim().to_string(),
        };
        t = format!("{head}{}", &t[(end + 1).min(t.len())..]);
    }
    let mut out = String::with_capacity(t.len());
    let mut depth = 0;
    for c in t.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_whitespace() => {}
            c => out.push(c),
        }
    }
    while out.contains("::::") {
        out = out.replace("::::", "::");
    }
    out.trim_matches(':').to_string()
}

fn last_path_seg(p: &str) -> &str {
    p.rsplit("::").next().unwrap_or(p)
}

/// Replaces the `Self` type by the implementing type.
fn subst_self(t: &str, self_ty: Option<&str>) -> String {
    let Some(s) = self_ty else { return t.to_string() };
    if !t.contains("Self") {
        return t.to_string();
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find("Self") {
        let before_ok = rest[..i].chars().next_back().map(|c| !ident(c)).unwrap_or(true);
        let after_ok = rest[i + 4..].chars().next().map(|c| !ident(c)).unwrap_or(true);
        out.push_str(&rest[..i]);
        out.push_str(if before_ok && after_ok { s } else { "Self" });
        rest = &rest[i + 4..];
    }
    out.push_str(rest);
    out
}

/// `T` of `Result<T, E>` / `Option<T>`; deferred member types get the `!await` unwrap mark,
/// resolved by the semantic layer with [`unwrap_result_type`].
fn unwrap_type(t: String) -> String {
    if t.starts_with("@mt#") {
        if t.ends_with("!await") {
            t
        } else {
            format!("{t}!await")
        }
    } else {
        unwrap_result_type(&t)
    }
}

/// `Result<T, E>` / `Option<T>` (through references) → `T`; any other type unchanged.
pub fn unwrap_result_type(t: &str) -> String {
    let s = strip_rust_indirection(t);
    let base = s.split('<').next().unwrap_or(s).trim();
    if matches!(last_path_seg(base), "Result" | "Option" | "LockResult") {
        if let Some(a) = generic_args(s).into_iter().next() {
            return a;
        }
    }
    t.to_string()
}

/// `E` of `Result<T, E>` (through references).
pub fn result_error_type(t: &str) -> Option<String> {
    let s = strip_rust_indirection(t);
    let base = s.split('<').next().unwrap_or(s).trim();
    (last_path_seg(base) == "Result").then(|| generic_args(s).into_iter().nth(1)).flatten()
}

fn visibility(ctx: &Ctx, node: Node) -> (String, bool) {
    vis_of(ctx, named_children(node).into_iter().find(|c| c.kind() == "visibility_modifier"))
}

/// (visibility, exported) of a `visibility_modifier`: `pub` / `pub(crate)` … / none.
fn vis_of(ctx: &Ctx, modifier: Option<Node>) -> (String, bool) {
    match modifier.map(|v| ctx.text(v).trim()) {
        None => ("private".into(), false),
        Some("pub") => ("public".into(), true),
        Some(_) => ("internal".into(), false),
    }
}

/// Declaration text up to its body: `pub fn f(x: i32) -> u8`, `impl Display for Foo`.
fn signature(ctx: &Ctx, node: Node, body: Option<Node>) -> String {
    let end = body.map(|b| b.start_byte()).unwrap_or(node.end_byte());
    collapse_ws(ctx.src[node.start_byte()..end].trim().trim_end_matches(';'))
}

fn attr_of(ctx: &Ctx, item: Node) -> String {
    let inner = item.named_child(0).map(|a| ctx.text(a)).unwrap_or("");
    format!("#[{}]", collapse_ws(inner))
}

/// `#[get("/x")]` → `get("/x")`.
fn attr_body(a: &str) -> &str {
    a.trim_start_matches("#[").trim_start_matches("#![").trim_end_matches(']')
}

fn attr_head(a: &str) -> &str {
    attr_body(a).split(['(', '=', ' ']).next().unwrap_or("").trim()
}

fn is_cfg_test(a: &str) -> bool {
    let b = attr_body(a).replace(' ', "");
    b == "cfg(test)" || (b.starts_with("cfg(") && b.contains("test") && !b.contains("not(test)"))
}

/// Outer doc comments (`///`, `/** */`) above an item, across its attributes.
fn doc_of(ctx: &Ctx, node: Node) -> Option<String> {
    let mut lines: Vec<&str> = Vec::new();
    let mut cur = node.prev_sibling();
    let mut expected = node.start_position().row;
    while let Some(c) = cur {
        match c.kind() {
            "attribute_item" => {}
            "line_comment" | "block_comment" => {
                let t = ctx.text(c);
                let doc = (t.starts_with("///") && !t.starts_with("////")) || (t.starts_with("/**") && !t.starts_with("/**/"));
                if !doc || c.end_position().row + 1 < expected {
                    break;
                }
                lines.insert(0, t);
            }
            _ => break,
        }
        expected = c.start_position().row;
        cur = c.prev_sibling();
    }
    doc_text(&lines)
}

/// Inner doc comments (`//!`) at the top of the file.
fn inner_doc(ctx: &Ctx, root: Node) -> Option<String> {
    let mut lines = Vec::new();
    for c in named_children(root) {
        if !matches!(c.kind(), "line_comment" | "block_comment") {
            break;
        }
        let t = ctx.text(c);
        if t.starts_with("//!") || t.starts_with("/*!") {
            lines.push(t);
        } else if !lines.is_empty() {
            break;
        }
    }
    doc_text(&lines)
}

fn doc_text(lines: &[&str]) -> Option<String> {
    let mut text = String::new();
    for l in lines.iter().flat_map(|l| l.lines()) {
        let l = l
            .trim()
            .trim_start_matches("///")
            .trim_start_matches("//!")
            .trim_start_matches("/**")
            .trim_start_matches("/*!")
            .trim_end_matches("*/")
            .trim_start_matches('*')
            .trim();
        if l.is_empty() {
            if !text.is_empty() {
                break; // first paragraph only
            }
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(l);
    }
    (!text.is_empty()).then(|| truncate(&text, 280))
}

fn first_string_in_tt(ctx: &Ctx, tt: Node) -> Option<String> {
    let s = named_children(tt).into_iter().find(|c| matches!(c.kind(), "string_literal" | "raw_string_literal"))?;
    Some(string_value(s, ctx.src))
}

fn token_tree_of(node: Node) -> Option<Node> {
    named_children(node).into_iter().find(|c| c.kind() == "token_tree")
}

/// String value of an argument: a literal, `&"x"`, `"x".to_string()` or `format!("…", …)`.
fn string_arg(ctx: &Ctx, node: Node) -> Option<String> {
    match node.kind() {
        "string_literal" | "raw_string_literal" => Some(string_value(node, ctx.src)),
        "reference_expression" => node.child_by_field_name("value").and_then(|v| string_arg(ctx, v)),
        "macro_invocation" => {
            let m = node.child_by_field_name("macro").map(|m| ctx.text(m)).unwrap_or("");
            if last_path_seg(m) != "format" {
                return None;
            }
            let tt = token_tree_of(node)?;
            let s = named_children(tt).into_iter().find(|c| c.kind() == "string_literal")?;
            Some(template_text(s, ctx.src))
        }
        "call_expression" => {
            // "x".to_string() / String::from("x")
            let f = node.child_by_field_name("function")?;
            match f.kind() {
                "field_expression" => f.child_by_field_name("value").and_then(|v| string_arg(ctx, v)),
                _ => node.child_by_field_name("arguments").and_then(|a| a.named_child(0)).and_then(|a| string_arg(ctx, a)),
            }
        }
        _ => None,
    }
}

fn arg_nodes(args: Option<Node>) -> Vec<Node> {
    args.map(named_children)
        .unwrap_or_default()
        .into_iter()
        .filter(|c| !matches!(c.kind(), "attribute_item" | "line_comment" | "block_comment"))
        .collect()
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

fn collect_struct_fields(ctx: &Ctx, root: Node, out: &mut HashMap<String, HashMap<String, Option<String>>>) {
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.kind() == "struct_item" || n.kind() == "union_item" {
            let Some(name) = n.child_by_field_name("name").map(|x| ctx.text(x).to_string()) else { continue };
            let mut fields = HashMap::new();
            if let Some(body) = n.child_by_field_name("body") {
                if body.kind() == "field_declaration_list" {
                    for f in named_children(body).into_iter().filter(|f| f.kind() == "field_declaration") {
                        if let Some(fname) = f.child_by_field_name("name") {
                            let ty = f.child_by_field_name("type").map(|t| subst_self(ctx.text(t), Some(&name)));
                            fields.insert(ctx.text(fname).to_string(), ty);
                        }
                    }
                } else {
                    let mut i = 0;
                    for t in named_children(body) {
                        if !matches!(t.kind(), "attribute_item" | "visibility_modifier" | "line_comment" | "block_comment") {
                            fields.insert(i.to_string(), Some(ctx.text(t).to_string()));
                            i += 1;
                        }
                    }
                }
            }
            out.entry(name).or_insert(fields);
            continue;
        }
        if matches!(n.kind(), "token_tree" | "function_item") {
            continue;
        }
        let mut c = n.walk();
        stack.extend(n.named_children(&mut c));
    }
}

fn is_item(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_signature_item"
            | "struct_item"
            | "union_item"
            | "enum_item"
            | "trait_item"
            | "impl_item"
            | "mod_item"
            | "use_declaration"
            | "const_item"
            | "static_item"
            | "type_item"
            | "associated_type"
            | "macro_definition"
            | "foreign_mod_item"
            | "extern_crate_declaration"
    )
}

/// Items of a file / module / trait / impl body, or the statements of a block, with their
/// outer attributes.
fn visit_items<'t>(ctx: &mut Ctx, st: &mut State<'t>, container: Node<'t>) {
    let block = container.kind() == "block";
    let mut attrs: Vec<String> = Vec::new();
    for ch in named_children(container) {
        match ch.kind() {
            "attribute_item" => attrs.push(attr_of(ctx, ch)),
            "inner_attribute_item" => {
                if is_cfg_test(&attr_of(ctx, ch)) {
                    st.in_test = true;
                }
            }
            "line_comment" | "block_comment" => {}
            k if is_item(k) => {
                let a = std::mem::take(&mut attrs);
                visit_item(ctx, st, ch, a);
            }
            // Item-position macros (`lazy_static! { … }`) declare nothing we can see.
            "macro_invocation" if !block => attrs.clear(),
            _ => {
                attrs.clear();
                walk(ctx, st, ch);
            }
        }
    }
}

fn visit_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let prev_test = st.in_test;
    if attrs.iter().any(|a| is_cfg_test(a)) {
        st.in_test = true;
    }
    match node.kind() {
        "use_declaration" => use_decl(ctx, st, node),
        "extern_crate_declaration" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                let local = node.child_by_field_name("alias").map(|a| ctx.text(a).to_string()).unwrap_or_else(|| name.clone());
                ctx.facts.imports.push(Import {
                    module: name,
                    imported: None,
                    local: Some(local),
                    kind: ImportKind::Namespace,
                    line: line_of(node),
                });
            }
        }
        "mod_item" => mod_item(ctx, st, node, &attrs),
        "struct_item" | "union_item" => struct_item(ctx, st, node, attrs),
        "enum_item" => enum_item(ctx, st, node, attrs),
        "trait_item" => trait_item(ctx, st, node, attrs),
        "impl_item" => impl_item(ctx, st, node, attrs),
        "function_item" | "function_signature_item" => function(ctx, st, node, attrs),
        "const_item" | "static_item" => const_item(ctx, st, node, attrs),
        "type_item" | "associated_type" => type_item(ctx, st, node, attrs),
        "macro_definition" => {
            if let Some(n) = node.child_by_field_name("name") {
                let name = ctx.text(n).to_string();
                let mut s = NewSymbol::new(SymbolKind::Function, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
                s.signature = format!("macro_rules! {name}");
                s.is_exported = attrs.iter().any(|a| attr_head(a) == "macro_export");
                s.visibility = if s.is_exported { "public".into() } else { "internal".into() };
                s.attrs = attrs;
                s.modifiers = vec!["macro".into()];
                s.docstring = doc_of(ctx, node);
                s.body_hash = token_hash(node, ctx.src);
                s.is_test = st.in_test;
                let qname = ctx.child_qname(&name);
                let id = ctx.add_symbol(s);
                ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Function), qname, is_function: true, ..Default::default() });
                macro_body_calls(ctx, node);
                ctx.pop_scope();
            }
        }
        "foreign_mod_item" => {
            if let Some(b) = node.child_by_field_name("body") {
                visit_items(ctx, st, b);
            }
        }
        _ => {}
    }
    st.in_test = prev_test;
}

/// Calls written in the expansions of a `macro_rules!` (token trees, never parsed): `f(…)` and
/// `path::f(…)` tokens, attributed to the macro.
fn macro_body_calls(ctx: &mut Ctx, def: Node) {
    let bodies =
        named_children(def).into_iter().filter(|r| r.kind() == "macro_rule").filter_map(|r| r.child_by_field_name("right")).collect();
    token_calls(ctx, bodies);
}

/// `f(…)` / `path::f(…)` call tokens in unparsed token trees.
fn token_calls(ctx: &mut Ctx, trees: Vec<Node>) {
    let mut stack = trees;
    while let Some(tt) = stack.pop() {
        let mut c = tt.walk();
        let toks: Vec<Node> = tt.children(&mut c).collect();
        for (i, t) in toks.iter().enumerate() {
            if t.kind() == "token_tree" {
                stack.push(*t);
                continue;
            }
            if t.kind() != "identifier" {
                continue;
            }
            // Next token after an optional turbofish (`f::<T>(`).
            let mut k = i + 1;
            if toks.get(k).map(|n| ctx.text(*n) == "::").unwrap_or(false) && toks.get(k + 1).map(|n| ctx.text(*n) == "<").unwrap_or(false) {
                let mut depth = 0;
                k += 1;
                while let Some(n) = toks.get(k) {
                    match ctx.text(*n) {
                        "<" => depth += 1,
                        ">" => depth -= 1,
                        ">>" => depth -= 2,
                        _ => {}
                    }
                    k += 1;
                    if depth <= 0 {
                        break;
                    }
                }
            }
            let called = toks.get(k).map(|n| n.kind() == "token_tree" && ctx.text(*n).starts_with('(')).unwrap_or(false);
            let prev = |k: usize| i.checked_sub(k).and_then(|j| toks.get(j)).map(|n| ctx.text(*n)).unwrap_or("");
            if !called || matches!(prev(1), "." | "$" | "fn") {
                continue;
            }
            // `a::b::f(` → receiver `a::b::`.
            let mut path = Vec::new();
            let mut j = i;
            while j >= 2 && ctx.text(toks[j - 1]) == "::" && toks[j - 2].kind() == "identifier" {
                path.insert(0, ctx.text(toks[j - 2]));
                j -= 2;
            }
            let name = ctx.text(*t);
            let receiver = path_receiver(&path.join("::"));
            if !matches!(name, "Some" | "Ok" | "Err" | "Box") {
                ctx.push_ref(RefKind::Call, name, receiver, None, None, line_of(*t), None);
            }
        }
    }
}

/// Rewrites a path written inside inline modules (`mod a { mod b { … } }`, `mod_path` =
/// `[a, b]`) relative to the file's module: `self::x` → `self::a::b::x`, `super::x` →
/// `self::a::x`, and supers beyond the inline modules stay `super`.
fn relative_to_file(segs: Vec<String>, mod_path: &[String]) -> Vec<String> {
    if mod_path.is_empty() {
        return segs;
    }
    match segs.first().map(|s| s.as_str()) {
        Some("self") => ["self".to_string()].into_iter().chain(mod_path.iter().cloned()).chain(segs.into_iter().skip(1)).collect(),
        Some("super") => {
            let ups = segs.iter().take_while(|s| *s == "super").count();
            let rest = segs.into_iter().skip(ups);
            if ups <= mod_path.len() {
                ["self".to_string()].into_iter().chain(mod_path[..mod_path.len() - ups].iter().cloned()).chain(rest).collect()
            } else {
                std::iter::repeat_n("super".to_string(), ups - mod_path.len()).chain(rest).collect()
            }
        }
        _ => segs,
    }
}

fn path_segs(text: &str) -> Vec<String> {
    clean_path(text).split("::").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Flattens a use tree into (path, alias, is_glob) entries.
fn use_tree(ctx: &Ctx, node: Node, prefix: &[String], out: &mut Vec<(Vec<String>, Option<String>, bool)>) {
    let join = |extra: Vec<String>| -> Vec<String> { prefix.iter().cloned().chain(extra).collect() };
    match node.kind() {
        "identifier" | "crate" | "self" | "super" | "scoped_identifier" | "metavariable" => {
            out.push((join(path_segs(ctx.text(node))), None, false))
        }
        "use_as_clause" => {
            let path = node.child_by_field_name("path").map(|p| path_segs(ctx.text(p))).unwrap_or_default();
            let alias = node.child_by_field_name("alias").map(|a| ctx.text(a).to_string());
            out.push((join(path), alias, false));
        }
        "use_wildcard" => {
            let path = node.named_child(0).map(|p| path_segs(ctx.text(p))).unwrap_or_default();
            out.push((join(path), None, true));
        }
        "scoped_use_list" => {
            let path = node.child_by_field_name("path").map(|p| path_segs(ctx.text(p))).unwrap_or_default();
            let full = join(path);
            if let Some(list) = node.child_by_field_name("list") {
                for item in named_children(list) {
                    use_tree(ctx, item, &full, out);
                }
            }
        }
        "use_list" => {
            for item in named_children(node) {
                use_tree(ctx, item, prefix, out);
            }
        }
        _ => {}
    }
}

fn use_decl(ctx: &mut Ctx, st: &mut State, node: Node) {
    let Some(arg) = node.child_by_field_name("argument") else { return };
    let public = visibility(ctx, node).1;
    let mut entries = Vec::new();
    use_tree(ctx, arg, &[], &mut entries);
    let line = line_of(node);
    for (segs, alias, glob) in entries {
        let mut segs = relative_to_file(segs, &st.mod_path);
        match segs.first().map(|s| s.as_str()) {
            Some("axum") => st.web = Some("axum"),
            Some("actix_web") => st.web = Some("actix"),
            Some("rocket") => st.web = Some("rocket"),
            Some("warp") => st.web = Some("warp"),
            Some("poem") => st.web = Some("poem"),
            _ => {}
        }
        if glob {
            // `pub use x::*` re-exports everything; a private glob only brings names into scope.
            let local = if public { None } else { Some("*".to_string()) };
            ctx.facts.imports.push(Import { module: segs.join("::"), imported: None, local, kind: ImportKind::Namespace, line });
            continue;
        }
        if segs.len() > 1 && segs.last().map(|s| s == "self").unwrap_or(false) {
            segs.pop(); // `use foo::{self}` imports the module itself
        }
        let Some(last) = segs.last().cloned() else { continue };
        let local = alias.unwrap_or_else(|| last.clone());
        if segs.len() == 1 {
            ctx.facts.imports.push(Import { module: last, imported: None, local: Some(local), kind: ImportKind::Namespace, line });
        } else {
            let module = segs[..segs.len() - 1].join("::");
            ctx.facts.imports.push(Import { module, imported: Some(last), local: Some(local), kind: ImportKind::Named, line });
        }
    }
}

fn mod_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: &[String]) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    match node.child_by_field_name("body") {
        Some(body) => {
            // Inline module: its items belong to this file's module, qualified by its name.
            let scope =
                Scope { symbol_id: ctx.enclosing(), kind: Some(SymbolKind::Module), qname: ctx.child_qname(&name), ..Default::default() };
            ctx.push_scope(scope);
            st.mod_path.push(name.clone());
            if attrs.iter().any(|a| is_cfg_test(a)) {
                st.in_test = true;
            }
            visit_items(ctx, st, body);
            st.mod_path.pop();
            ctx.pop_scope();
        }
        None => {
            // `mod x;` → the file `x.rs` / `x/mod.rs` next to this module.
            ctx.facts.imports.push(Import {
                module: relative_to_file(vec!["self".into(), name.clone()], &st.mod_path).join("::"),
                imported: None,
                local: Some(name),
                kind: ImportKind::Namespace,
                line: line_of(node),
            });
        }
    }
}

/// Generic parameters (and `where` clauses) → their first trait bound.
fn type_params(ctx: &Ctx, node: Node) -> HashMap<String, Option<String>> {
    let mut out = HashMap::new();
    let first_bound = |b: Option<Node>| -> Option<String> {
        b.and_then(|b| named_children(b).into_iter().find(|x| x.kind() != "lifetime")).map(|t| ctx.text(t).to_string())
    };
    if let Some(tp) = node.child_by_field_name("type_parameters") {
        for p in named_children(tp) {
            if p.kind() == "type_parameter" {
                if let Some(n) = p.child_by_field_name("name") {
                    out.insert(ctx.text(n).to_string(), first_bound(p.child_by_field_name("bounds")));
                }
            }
        }
    }
    if let Some(w) = named_children(node).into_iter().find(|c| c.kind() == "where_clause") {
        for p in named_children(w) {
            if let Some(l) = p.child_by_field_name("left").filter(|l| l.kind() == "type_identifier") {
                let b = first_bound(p.child_by_field_name("bounds"));
                let e = out.entry(ctx.text(l).to_string()).or_insert(None);
                if e.is_none() {
                    *e = b;
                }
            }
        }
    }
    out
}

/// Records `USES_TYPE` facts for the project-looking types named in a type expression.
fn type_uses(ctx: &mut Ctx, st: &mut State, ty: Node) {
    let mut stack = vec![ty];
    let mut names = Vec::new();
    while let Some(n) = stack.pop() {
        match n.kind() {
            "type_identifier" => names.push((ctx.text(n).to_string(), line_of(n))),
            "scoped_type_identifier" => names.push((clean_path(ctx.text(n)), line_of(n))),
            "lifetime" | "primitive_type" => {}
            _ => {
                let mut c = n.walk();
                stack.extend(n.named_children(&mut c));
            }
        }
    }
    let enclosing = ctx.enclosing();
    for (name, line) in names {
        let short = last_path_seg(&name);
        if COMMON_TYPES.contains(&short) || ctx.scopes.iter().any(|s| s.tparams.contains_key(short)) {
            continue;
        }
        if st.type_uses.insert((enclosing.clone(), name.clone())) {
            ctx.push_ref(RefKind::TypeUse, &name, None, None, None, line, None);
        }
    }
}

fn struct_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let body = node.child_by_field_name("body");
    let brace = body.filter(|b| b.kind() == "field_declaration_list");
    let (vis, exported) = visibility(ctx, node);
    let mut s = NewSymbol::new(SymbolKind::Struct, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, brace);
    s.visibility = vis;
    s.is_exported = exported;
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    if node.kind() == "union_item" {
        s.modifiers.push("union".into());
    }
    s.attrs = attrs;
    let id = ctx.add_symbol(s);
    let fields = st.struct_fields.get(&name).cloned().unwrap_or_default();
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Struct),
        qname,
        fields,
        tparams: type_params(ctx, node),
        ..Default::default()
    });
    if let Some(b) = body {
        let mut attrs: Vec<String> = Vec::new();
        let mut index = 0;
        for f in named_children(b) {
            match f.kind() {
                "attribute_item" => attrs.push(attr_of(ctx, f)),
                "field_declaration" => {
                    let Some(fname) = f.child_by_field_name("name").map(|x| ctx.text(x).to_string()) else { continue };
                    let ty = f.child_by_field_name("type");
                    field_symbol(ctx, st, f, &fname, ty, std::mem::take(&mut attrs), Some(&name));
                }
                "visibility_modifier" | "line_comment" | "block_comment" => {}
                _ if b.kind() == "ordered_field_declaration_list" => {
                    // Tuple struct field: the type node itself.
                    let fname = index.to_string();
                    index += 1;
                    field_symbol(ctx, st, f, &fname, Some(f), std::mem::take(&mut attrs), Some(&name));
                }
                _ => {}
            }
        }
    }
    ctx.pop_scope();
}

fn field_symbol(ctx: &mut Ctx, st: &mut State, node: Node, name: &str, ty: Option<Node>, attrs: Vec<String>, owner: Option<&str>) {
    let ty_text = ty.map(|t| subst_self(ctx.text(t), owner));
    let (vis, exported) = if node.kind() == "field_declaration" {
        visibility(ctx, node)
    } else {
        // Tuple field `pub u32`: the visibility is the type node's preceding sibling.
        vis_of(ctx, node.prev_named_sibling().filter(|p| p.kind() == "visibility_modifier"))
    };
    let mut s = NewSymbol::new(SymbolKind::Field, name, ctx.child_qname(name), line_of(node), end_line_of(node));
    s.signature = collapse_ws(ctx.text(node).trim_end_matches(','));
    s.type_ref = ty_text;
    s.visibility = vis;
    s.is_exported = exported;
    s.attrs = attrs;
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    ctx.add_symbol(s);
    if let Some(t) = ty {
        type_uses(ctx, st, t);
    }
}

fn enum_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let body = node.child_by_field_name("body");
    let (vis, exported) = visibility(ctx, node);
    let mut s = NewSymbol::new(SymbolKind::Enum, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, body);
    s.visibility = vis;
    s.is_exported = exported;
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    s.attrs = attrs;
    let id = ctx.add_symbol(s);
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope { symbol_id: id, kind: Some(SymbolKind::Enum), qname, tparams: type_params(ctx, node), ..Default::default() });
    let prev_self = st.self_type.replace(name.clone());
    if let Some(b) = body {
        let mut attrs: Vec<String> = Vec::new();
        for v in named_children(b) {
            match v.kind() {
                "attribute_item" => attrs.push(attr_of(ctx, v)),
                "enum_variant" => {
                    let Some(vn) = v.child_by_field_name("name").map(|x| ctx.text(x).to_string()) else { continue };
                    let mut vs = NewSymbol::new(SymbolKind::Const, &vn, ctx.child_qname(&vn), line_of(v), end_line_of(v));
                    vs.signature = collapse_ws(ctx.text(v));
                    vs.value_text = v.child_by_field_name("value").map(|x| ctx.text(x).to_string());
                    vs.type_ref = Some(name.clone());
                    vs.is_exported = exported;
                    vs.attrs = std::mem::take(&mut attrs);
                    vs.docstring = doc_of(ctx, v);
                    vs.body_hash = token_hash(v, ctx.src);
                    vs.is_test = st.in_test;
                    ctx.add_symbol(vs);
                    if let Some(fields) = v.child_by_field_name("body") {
                        type_uses(ctx, st, fields);
                    }
                }
                _ => {}
            }
        }
    }
    st.self_type = prev_self;
    ctx.pop_scope();
}

fn trait_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let body = node.child_by_field_name("body");
    let (vis, exported) = visibility(ctx, node);
    let mut s = NewSymbol::new(SymbolKind::Interface, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, body);
    s.visibility = vis;
    s.is_exported = exported;
    s.is_abstract = true;
    s.bases = node
        .child_by_field_name("bounds")
        .map(|b| named_children(b).into_iter().filter(|x| x.kind() != "lifetime").map(|x| ctx.text(x).to_string()).collect())
        .unwrap_or_default();
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    s.attrs = attrs;
    let id = ctx.add_symbol(s);
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Interface),
        qname,
        tparams: type_params(ctx, node),
        ..Default::default()
    });
    let prev_self = st.self_type.replace(name);
    let prev_ti = std::mem::replace(&mut st.in_trait_impl, false);
    if let Some(b) = body {
        visit_items(ctx, st, b);
    }
    st.self_type = prev_self;
    st.in_trait_impl = prev_ti;
    ctx.pop_scope();
}

fn impl_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(ty) = node.child_by_field_name("type") else { return };
    let ty_text = collapse_ws(ctx.text(ty));
    let self_ty = clean_path(strip_rust_indirection(&ty_text));
    let name = last_path_seg(&self_ty).to_string();
    if name.is_empty() {
        return;
    }
    let tr = node.child_by_field_name("trait").map(|t| collapse_ws(ctx.text(t)));
    let body = node.child_by_field_name("body");
    let mut s = NewSymbol::new(SymbolKind::Impl, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, body);
    s.type_ref = Some(ty_text);
    s.bases = tr.iter().map(|t| t.trim_start_matches('!').to_string()).collect();
    s.modifiers.push(if tr.is_some() { "trait_impl".into() } else { "inherent".into() });
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    s.attrs = attrs;
    let id = ctx.add_symbol(s);
    let fields = st.struct_fields.get(&name).cloned().unwrap_or_default();
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope {
        symbol_id: id,
        kind: Some(SymbolKind::Impl),
        qname,
        fields,
        tparams: type_params(ctx, node),
        ..Default::default()
    });
    if let Some(t) = node.child_by_field_name("trait") {
        type_uses(ctx, st, t);
    }
    let prev_self = st.self_type.replace(self_ty);
    let prev_ti = std::mem::replace(&mut st.in_trait_impl, tr.is_some());
    if let Some(b) = body {
        visit_items(ctx, st, b);
    }
    st.self_type = prev_self;
    st.in_trait_impl = prev_ti;
    ctx.pop_scope();
}

fn const_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let ty = node.child_by_field_name("type");
    let ty_text = ty.map(|t| subst_self(ctx.text(t), st.self_type.as_deref()));
    let value = node.child_by_field_name("value");
    let is_mut = named_children(node).iter().any(|c| c.kind() == "mutable_specifier");
    let kind = if is_mut { SymbolKind::Variable } else { SymbolKind::Const };
    let (vis, exported) = visibility(ctx, node);
    let in_trait = ctx.scopes.last().and_then(|s| s.kind) == Some(SymbolKind::Interface);
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, value).trim_end_matches('=').trim().to_string();
    s.type_ref = ty_text.clone();
    s.value_text = value.map(|v| ctx.text(v).to_string());
    s.visibility = if in_trait || st.in_trait_impl { "public".into() } else { vis };
    s.is_exported = exported;
    s.is_static = true;
    if node.kind() == "static_item" {
        s.modifiers.push(if is_mut { "static mut".into() } else { "static".into() });
    }
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    s.attrs = attrs;
    if ctx.scopes.last().and_then(|s| s.kind) == Some(SymbolKind::Module) {
        ctx.declare_local(&name, ty_text);
    }
    let id = ctx.add_symbol(s);
    let qname = ctx.child_qname(&name);
    ctx.push_scope(Scope { symbol_id: id, qname, ..Default::default() });
    if let Some(t) = ty {
        type_uses(ctx, st, t);
    }
    if let Some(v) = value {
        walk(ctx, st, v);
    }
    ctx.pop_scope();
}

fn type_item<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let ty = node.child_by_field_name("type");
    let (vis, exported) = visibility(ctx, node);
    let in_trait = ctx.scopes.last().and_then(|s| s.kind) == Some(SymbolKind::Interface);
    let mut s = NewSymbol::new(SymbolKind::TypeAlias, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, None);
    s.type_ref = ty.map(|t| subst_self(ctx.text(t), st.self_type.as_deref()));
    s.visibility = if in_trait || st.in_trait_impl { "public".into() } else { vis };
    s.is_exported = exported;
    s.is_abstract = in_trait && ty.is_none();
    s.docstring = doc_of(ctx, node);
    s.body_hash = token_hash(node, ctx.src);
    s.is_test = st.in_test;
    s.attrs = attrs;
    ctx.add_symbol(s);
    if let Some(t) = ty {
        type_uses(ctx, st, t);
    }
}

/// (params, takes self, locals)
fn parse_params(ctx: &Ctx, st: &State, node: Option<Node>) -> (ParamInfo, bool, Vec<(String, Option<String>)>) {
    let Some(node) = node else { return (ParamInfo::default(), false, Vec::new()) };
    let mut list = Vec::new();
    let mut locals = Vec::new();
    let mut has_self = false;
    for p in named_children(node) {
        match p.kind() {
            "self_parameter" => {
                has_self = true;
                locals.push(("self".to_string(), st.self_type.clone()));
            }
            "parameter" => {
                let ty = p.child_by_field_name("type").map(|t| subst_self(ctx.text(t), st.self_type.as_deref()));
                let Some(pat) = p.child_by_field_name("pattern") else { continue };
                let name = ctx.text(pat).to_string();
                if pat.kind() == "identifier" {
                    locals.push((name.clone(), ty.clone()));
                } else {
                    let mut ids = Vec::new();
                    pattern_idents(ctx, pat, &mut ids);
                    locals.extend(ids.into_iter().map(|i| (i, None)));
                }
                list.push(Param { name, ty, optional: false, variadic: false });
            }
            "variadic_parameter" => list.push(Param { name: "...".into(), ty: None, optional: false, variadic: true }),
            _ => {}
        }
    }
    (ParamInfo::from_list(list), has_self, locals)
}

/// Identifiers bound by a pattern.
fn pattern_idents(ctx: &Ctx, node: Node, out: &mut Vec<String>) {
    match node.kind() {
        "identifier" | "shorthand_field_identifier" => out.push(ctx.text(node).to_string()),
        "scoped_identifier" | "type_identifier" | "scoped_type_identifier" | "generic_type" | "string_literal" | "integer_literal" => {}
        "tuple_struct_pattern" | "struct_pattern" => {
            let ty = node.child_by_field_name("type");
            for c in named_children(node) {
                if Some(c) != ty {
                    pattern_idents(ctx, c, out);
                }
            }
        }
        "field_pattern" => match node.child_by_field_name("pattern") {
            Some(p) => pattern_idents(ctx, p, out),
            None => {
                if let Some(n) = node.child_by_field_name("name") {
                    out.push(ctx.text(n).to_string());
                }
            }
        },
        _ => {
            for c in named_children(node) {
                pattern_idents(ctx, c, out);
            }
        }
    }
}

fn declare_pattern(ctx: &mut Ctx, node: Node, ty: Option<String>) {
    let target =
        if node.kind() == "mut_pattern" { node.named_child((node.named_child_count() as u32).saturating_sub(1)) } else { Some(node) };
    if let Some(t) = target.filter(|t| t.kind() == "identifier") {
        let name = ctx.text(t).to_string();
        ctx.declare_local(&name, ty);
        return;
    }
    let mut ids = Vec::new();
    pattern_idents(ctx, node, &mut ids);
    for i in ids {
        ctx.declare_local(&i, None);
    }
}

fn function<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>, attrs: Vec<String>) {
    let Some(n) = node.child_by_field_name("name") else { return };
    let name = ctx.text(n).to_string();
    let scope_kind = ctx.scopes.last().and_then(|s| s.kind);
    let in_impl = scope_kind == Some(SymbolKind::Impl);
    let in_trait = scope_kind == Some(SymbolKind::Interface);
    let (params, has_self, locals) = parse_params(ctx, st, node.child_by_field_name("parameters"));
    let ret = node.child_by_field_name("return_type").map(|t| subst_self(ctx.text(t), st.self_type.as_deref()));
    let body = node.child_by_field_name("body");
    let fmods: Vec<String> = named_children(node)
        .into_iter()
        .filter(|c| c.kind() == "function_modifiers")
        .flat_map(|m| ctx.text(m).split_whitespace().map(|w| w.trim_matches('"').to_string()).collect::<Vec<_>>())
        .collect();
    let is_async = fmods.iter().any(|m| m == "async");
    let kind = if in_impl || in_trait { SymbolKind::Method } else { SymbolKind::Function };
    let (vis, exported) = visibility(ctx, node);
    let mut s = NewSymbol::new(kind, &name, ctx.child_qname(&name), line_of(node), end_line_of(node));
    s.signature = signature(ctx, node, body);
    s.is_async = is_async;
    s.is_static = (in_impl || in_trait) && !has_self;
    s.is_abstract = in_trait && body.is_none();
    s.visibility = if in_trait || st.in_trait_impl { "public".into() } else { vis };
    s.is_exported = exported || in_trait || st.in_trait_impl;
    s.type_ref = ret;
    s.params = Some(params);
    s.docstring = doc_of(ctx, node);
    s.complexity = cyclomatic(node, ctx.src, ctx.lang);
    s.body_hash = token_hash(node, ctx.src);
    for m in ["async", "unsafe", "const", "extern"] {
        if fmods.iter().any(|x| x == m) {
            s.modifiers.push(m.into());
        }
    }
    if st.in_trait_impl {
        s.modifiers.push("override".into());
    }
    if body.is_none() && !in_trait {
        s.modifiers.push("extern".into());
    }
    let heads: Vec<&str> = attrs.iter().map(|a| attr_head(a)).collect();
    s.is_test = st.in_test || heads.iter().any(|h| TEST_ATTRS.contains(h) || h.ends_with("::test"));

    // ---- Entry points ----
    let module_level = !in_impl && !in_trait && ctx.scopes.iter().all(|sc| sc.kind == Some(SymbolKind::Module));
    let mut route: Option<(String, String)> = None;
    if heads.iter().any(|h| h.ends_with("::main") || *h == "main") || (name == "main" && module_level && ctx.scopes.len() == 1) {
        s.is_entrypoint = true;
        s.entry_kind = Some("main".into());
    }
    for a in &attrs {
        let head = attr_head(a);
        let short = last_path_seg(head);
        if ROUTE_ATTRS.contains(&short) {
            let body_text = attr_body(a);
            if let Some(path) = first_quoted(body_text).filter(|p| p.starts_with('/')) {
                let method = if short == "route" {
                    body_text.find("method").and_then(|i| first_quoted(&body_text[i..])).unwrap_or_else(|| "GET".into())
                } else {
                    short.to_string()
                };
                route = Some((method.to_ascii_uppercase(), path));
            }
        }
        let entry = match short {
            "no_mangle" | "export_name" => Some("ffi export"),
            "wasm_bindgen" => Some("wasm export"),
            "command" if head.contains("tauri") => Some("tauri command"),
            "pyfunction" | "pymethods" | "pyclass" => Some("python binding"),
            "napi" => Some("node binding"),
            "proc_macro" | "proc_macro_derive" | "proc_macro_attribute" => Some("proc macro"),
            "handler" | "endpoint" | "oai" => Some("web handler"),
            _ => None,
        };
        if let Some(e) = entry {
            s.is_entrypoint = true;
            s.entry_kind = Some(e.into());
        }
    }
    if s.modifiers.iter().any(|m| m == "extern") && exported && body.is_some() {
        s.is_entrypoint = true;
        s.entry_kind = Some("ffi export".into());
    }
    if let Some((m, p)) = &route {
        s.is_entrypoint = true;
        s.entry_kind = Some(format!("route {m} {p}"));
    }
    s.attrs = attrs;
    let line = line_of(node);
    let id = ctx.add_symbol(s);
    if let Some((m, p)) = route {
        let fw = st.web.unwrap_or("rust");
        ctx.push_route(&id, &m, &p, fw, line);
    }

    let mut scope = Scope {
        symbol_id: id,
        kind: Some(kind),
        qname: ctx.child_qname(&name),
        is_function: true,
        tparams: type_params(ctx, node),
        ..Default::default()
    };
    for (n, t) in locals {
        scope.locals.insert(n, t);
    }
    ctx.push_scope(scope);
    if let Some(ps) = node.child_by_field_name("parameters") {
        for p in named_children(ps) {
            if let Some(t) = p.child_by_field_name("type") {
                type_uses(ctx, st, t);
            }
        }
    }
    if let Some(r) = node.child_by_field_name("return_type") {
        type_uses(ctx, st, r);
    }
    let prev_in_ti = std::mem::replace(&mut st.in_trait_impl, false);
    if let Some(b) = body {
        walk(ctx, st, b);
    }
    st.in_trait_impl = prev_in_ti;
    ctx.pop_scope();
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

/// Static type of an expression, when it can be told from this file (concrete type text or a
/// deferred `@mt#` member type resolved by the semantic layer).
fn expr_type(ctx: &Ctx, st: &State, n: Node) -> Option<String> {
    match n.kind() {
        "self" => st.self_type.clone(),
        "identifier" => {
            let t = ctx.text(n);
            if ctx.is_local(t) || ctx.is_module_var(t) {
                ctx.subst_tparam(ctx.var_type(t))
            } else {
                None
            }
        }
        "parenthesized_expression" | "unary_expression" | "await_expression" => n.named_child(0).and_then(|c| expr_type(ctx, st, c)),
        "reference_expression" => n.child_by_field_name("value").and_then(|c| expr_type(ctx, st, c)),
        "try_expression" => n.named_child(0).and_then(|c| expr_type(ctx, st, c)).map(unwrap_type),
        "type_cast_expression" => n.child_by_field_name("type").map(|t| ctx.text(t).to_string()),
        "struct_expression" => {
            let name = n.child_by_field_name("name").map(|x| clean_path(ctx.text(x)))?;
            if name == "Self" {
                st.self_type.clone()
            } else {
                Some(name)
            }
        }
        "field_expression" => {
            let v = n.child_by_field_name("value")?;
            let f = n.child_by_field_name("field")?;
            if f.kind() != "field_identifier" {
                return None;
            }
            let f = ctx.text(f);
            if v.kind() == "self" {
                return ctx.field_type(f).or_else(|| Some(member_type_marker(None, f, false)));
            }
            let owner = expr_type(ctx, st, v)?;
            Some(member_type_marker(Some(&owner), f, false))
        }
        "call_expression" => {
            let mut f = n.child_by_field_name("function")?;
            if f.kind() == "generic_function" {
                f = f.child_by_field_name("function")?;
            }
            match f.kind() {
                "scoped_identifier" => {
                    let path = clean_path(ctx.text(f.child_by_field_name("path")?));
                    let name = ctx.text(f.child_by_field_name("name")?);
                    let owner = if path == "Self" { st.self_type.clone()? } else { path };
                    if !is_upper_ident(last_path_seg(&owner)) {
                        // `tree::get(&conn)`: return type of a function of a module.
                        return Some(member_type_marker(Some("@fn"), &format!("{owner}::{name}"), false));
                    }
                    if is_ctor_name(name) {
                        Some(owner)
                    } else {
                        Some(member_type_marker(Some(&owner), name, false))
                    }
                }
                "identifier" => {
                    let name = ctx.text(f);
                    if matches!(name, "Some" | "Ok" | "Err" | "Box") {
                        let a = n.child_by_field_name("arguments").and_then(|a| a.named_child(0))?;
                        return expr_type(ctx, st, a);
                    }
                    if is_upper_ident(name) {
                        Some(name.to_string())
                    } else if ctx.is_local(name) {
                        None
                    } else {
                        Some(member_type_marker(Some("@fn"), name, false))
                    }
                }
                "field_expression" => {
                    let m = ctx.text(f.child_by_field_name("field")?);
                    let v = f.child_by_field_name("value")?;
                    if IDENTITY_METHODS.contains(&m) {
                        return expr_type(ctx, st, v);
                    }
                    if UNWRAP_METHODS.contains(&m) {
                        return expr_type(ctx, st, v).map(unwrap_type);
                    }
                    if v.kind() == "self" {
                        return Some(member_type_marker(None, m, false));
                    }
                    let owner = expr_type(ctx, st, v)?;
                    Some(member_type_marker(Some(&owner), m, false))
                }
                _ => None,
            }
        }
        "index_expression" => {
            let t = n.named_child(0).and_then(|c| expr_type(ctx, st, c))?;
            if t.starts_with("@mt#") {
                None
            } else {
                element_type(&t)
            }
        }
        _ => None,
    }
}

/// Receiver text (without `?` / `.await`) and static type.
fn receiver_info(ctx: &Ctx, st: &State, r: Node) -> (String, Option<String>) {
    let text = ctx.text(r).replace(".await", "").replace('?', "");
    // Chains split over lines (`self\n    .index`) read as `self.index`.
    let text = if r.kind() == "self" { "self".to_string() } else { simplify_receiver(&text).replace(" .", ".") };
    (text, expr_type(ctx, st, r))
}

/// `crate::db::open` path receivers keep Rust syntax with a trailing `::`; `Self::f` is a call
/// on the enclosing type and `self::f` a plain call in this module.
fn path_receiver(path: &str) -> Option<String> {
    match path {
        "" | "self" => None,
        "Self" => Some("self".into()),
        p => Some(format!("{p}::")),
    }
}

fn is_type_node(kind: &str) -> bool {
    kind.ends_with("_type")
        || matches!(
            kind,
            "type_identifier"
                | "type_arguments"
                | "type_parameters"
                | "lifetime"
                | "label"
                | "mutable_specifier"
                | "token_tree"
                | "where_clause"
        )
}

fn walk<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>) {
    let kind = node.kind();
    match kind {
        "block" => visit_items(ctx, st, node),
        "call_expression" => call(ctx, st, node),
        "macro_invocation" => macro_call(ctx, st, node),
        "let_declaration" => {
            let value = node.child_by_field_name("value");
            let ty_node = node.child_by_field_name("type");
            let ty =
                ty_node.map(|t| subst_self(ctx.text(t), st.self_type.as_deref())).or_else(|| value.and_then(|v| expr_type(ctx, st, v)));
            if let Some(v) = value {
                walk(ctx, st, v);
            }
            if let Some(alt) = node.child_by_field_name("alternative") {
                walk(ctx, st, alt);
            }
            if let Some(p) = node.child_by_field_name("pattern") {
                declare_pattern(ctx, p, ty);
            }
            if let Some(t) = ty_node {
                type_uses(ctx, st, t);
            }
        }
        "assignment_expression" | "compound_assignment_expr" => {
            if let Some(r) = node.child_by_field_name("right") {
                walk(ctx, st, r);
            }
            if let Some(l) = node.child_by_field_name("left") {
                write_target(ctx, st, l);
            }
        }
        "field_expression" => {
            let Some(v) = node.child_by_field_name("value") else { return };
            let field = node.child_by_field_name("field").filter(|f| f.kind() == "field_identifier").map(|f| ctx.text(f).to_string());
            match field {
                Some(f) if v.kind() == "self" => {
                    let cls = ctx.class_name();
                    ctx.push_ref(RefKind::Read, &f, Some("self".into()), cls, None, line_of(node), None);
                }
                Some(f) => {
                    if let Some(t) = expr_type(ctx, st, v) {
                        let (rt, _) = receiver_info(ctx, st, v);
                        ctx.push_ref(RefKind::Read, &f, Some(rt), Some(t), None, line_of(node), None);
                    }
                    walk(ctx, st, v);
                }
                None => walk(ctx, st, v),
            }
        }
        "scoped_identifier" => {
            let path = node.child_by_field_name("path").map(|p| clean_path(ctx.text(p))).unwrap_or_default();
            let Some(name) = node.child_by_field_name("name").map(|n| ctx.text(n).to_string()) else { return };
            let rtype = if path == "Self" { st.self_type.clone() } else { None };
            ctx.push_ref(RefKind::Read, &name, path_receiver(&path), rtype, None, line_of(node), None);
        }
        "identifier" => {
            let name = ctx.text(node);
            if ctx.is_local(name) || matches!(name, "None" | "Some" | "Ok" | "Err" | "_") {
                return;
            }
            ctx.push_ref(RefKind::Read, name, None, None, None, line_of(node), None);
        }
        "self" => {}
        "closure_expression" => {
            if let Some(ps) = node.child_by_field_name("parameters") {
                for p in named_children(ps) {
                    let (pat, ty) = if p.kind() == "parameter" {
                        (p.child_by_field_name("pattern"), p.child_by_field_name("type").map(|t| ctx.text(t).to_string()))
                    } else {
                        (Some(p), None)
                    };
                    if let Some(pat) = pat {
                        declare_pattern(ctx, pat, ty);
                    }
                }
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
        }
        "for_expression" => {
            let value = node.child_by_field_name("value");
            let elem = value.and_then(|v| expr_type(ctx, st, v)).filter(|t| !t.starts_with("@mt#")).and_then(|t| element_type(&t));
            if let Some(v) = value {
                walk(ctx, st, v);
            }
            if let Some(p) = node.child_by_field_name("pattern") {
                declare_pattern(ctx, p, elem);
            }
            if let Some(b) = node.child_by_field_name("body") {
                walk(ctx, st, b);
            }
        }
        "match_arm" => {
            if let Some(p) = node.child_by_field_name("pattern") {
                for c in named_children(p) {
                    if Some(c) == p.child_by_field_name("condition") {
                        walk(ctx, st, c);
                    } else {
                        declare_pattern(ctx, c, None);
                    }
                }
            }
            if let Some(v) = node.child_by_field_name("value") {
                walk(ctx, st, v);
            }
        }
        "let_condition" => {
            let value = node.child_by_field_name("value");
            let value_ty = value.and_then(|v| expr_type(ctx, st, v));
            if let Some(v) = value {
                walk(ctx, st, v);
            }
            if let Some(p) = node.child_by_field_name("pattern") {
                let (binding, ty) = match p.kind() {
                    "identifier" => (Some(p), value_ty),
                    // `if let Some(x) = opt` / `Ok(x)`: the wrapped `T`; `Err(e)`: the `E` of a
                    // `Result<T, E>`; any other variant binds a field of unknown type.
                    "tuple_struct_pattern" if p.named_child_count() == 2 => {
                        let ctor =
                            p.child_by_field_name("type").map(|t| last_path_seg(&clean_path(ctx.text(t))).to_string()).unwrap_or_default();
                        let ty = match ctor.as_str() {
                            "Some" | "Ok" => value_ty.map(unwrap_type),
                            "Err" => value_ty.and_then(|t| {
                                if t.starts_with("@mt#") {
                                    // Deferred return type: the semantic layer takes its `E`.
                                    (!t.ends_with("!await")).then(|| format!("{t}!err"))
                                } else {
                                    result_error_type(&t)
                                }
                            }),
                            _ => None,
                        };
                        (p.named_child(1).filter(|b| b.kind() == "identifier"), ty)
                    }
                    _ => (None, None),
                };
                match binding {
                    Some(b) => {
                        let n = ctx.text(b).to_string();
                        ctx.declare_local(&n, ty);
                    }
                    None => declare_pattern(ctx, p, None),
                }
            }
        }
        "struct_expression" => {
            if let Some(n) = node.child_by_field_name("name") {
                let mut name = clean_path(ctx.text(n));
                if name == "Self" {
                    name = st.self_type.clone().unwrap_or(name);
                }
                ctx.push_ref(RefKind::New, &name, None, None, None, line_of(node), None);
            }
            if let Some(b) = node.child_by_field_name("body") {
                for f in named_children(b) {
                    match f.kind() {
                        "field_initializer" => {
                            if let Some(v) = f.child_by_field_name("value") {
                                walk(ctx, st, v);
                            }
                        }
                        _ => walk(ctx, st, f),
                    }
                }
            }
        }
        "shorthand_field_initializer" => {
            if let Some(i) = node.named_child((node.named_child_count() as u32).saturating_sub(1)) {
                walk(ctx, st, i);
            }
        }
        "await_expression" => {
            ctx.mark_await();
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
        "string_literal" | "raw_string_literal" => {
            let v = string_value(node, ctx.src);
            ctx.push_literal(&v, "string", node);
        }
        "integer_literal" | "float_literal" => {
            let v = ctx.text(node).to_string();
            ctx.push_literal(&v, "number", node);
        }
        "type_cast_expression" => {
            if let Some(v) = node.child_by_field_name("value") {
                walk(ctx, st, v);
            }
        }
        "generic_function" => {
            if let Some(f) = node.child_by_field_name("function") {
                walk(ctx, st, f);
            }
        }
        "attribute_item" | "inner_attribute_item" | "line_comment" | "block_comment" => {}
        k if is_item(k) => visit_item(ctx, st, node, Vec::new()),
        k if is_type_node(k) => {}
        _ => {
            for ch in named_children(node) {
                walk(ctx, st, ch);
            }
        }
    }
}

fn write_target<'t>(ctx: &mut Ctx, st: &mut State<'t>, left: Node<'t>) {
    let line = line_of(left);
    match left.kind() {
        "identifier" => {
            let name = ctx.text(left).to_string();
            if !ctx.is_local(&name) && ctx.is_module_var(&name) && ctx.in_function() {
                ctx.push_ref(RefKind::Write, &name, None, None, None, line, None);
                ctx.push_mutation(&name, "global_var", line);
            }
        }
        "field_expression" => {
            let Some(v) = left.child_by_field_name("value") else { return };
            let Some(f) = left.child_by_field_name("field").map(|f| ctx.text(f).to_string()) else { return };
            if v.kind() == "self" {
                let cls = ctx.class_name();
                ctx.push_ref(RefKind::Write, &f, Some("self".into()), cls.clone(), None, line, None);
                ctx.push_mutation(&format!("{}.{}", cls.unwrap_or_default(), f), "local_field", line);
            } else {
                let (rtext, rtype) = receiver_info(ctx, st, v);
                let owner = rtype.clone().filter(|t| !t.starts_with("@mt#")).map(|t| clean_type(&t)).unwrap_or_else(|| rtext.clone());
                ctx.push_ref(RefKind::Write, &f, Some(rtext), rtype, None, line, None);
                ctx.push_mutation(&format!("{owner}.{f}"), "object_field", line);
                walk(ctx, st, v);
            }
        }
        "index_expression" => {
            if let Some(base) = left.named_child(0) {
                if base.kind() == "identifier" || base.kind() == "field_expression" {
                    write_target(ctx, st, base);
                }
            }
            for ch in named_children(left) {
                walk(ctx, st, ch);
            }
        }
        _ => walk(ctx, st, left),
    }
}

/// Error type of `Err(…)`: `E::Variant`, `E::new(…)`, `E { … }`, `anyhow!(…)`.
fn error_type(ctx: &Ctx, st: &State, n: Node) -> Option<String> {
    let path_owner = |p: &str| -> Option<String> {
        let p = clean_path(p);
        let p = if p == "Self" { st.self_type.clone()? } else { p };
        is_upper_ident(last_path_seg(&p)).then_some(p)
    };
    match n.kind() {
        "scoped_identifier" => n.child_by_field_name("path").and_then(|p| path_owner(ctx.text(p))),
        "struct_expression" => n.child_by_field_name("name").and_then(|x| path_owner(ctx.text(x))),
        "call_expression" => {
            let f = n.child_by_field_name("function")?;
            match f.kind() {
                "scoped_identifier" => f.child_by_field_name("path").and_then(|p| path_owner(ctx.text(p))),
                "identifier" if is_upper_ident(ctx.text(f)) => Some(ctx.text(f).to_string()),
                "field_expression" if f.child_by_field_name("field").map(|x| ctx.text(x) == "into").unwrap_or(false) => {
                    f.child_by_field_name("value").and_then(|v| error_type(ctx, st, v))
                }
                _ => None,
            }
        }
        "macro_invocation" => {
            let m = n.child_by_field_name("macro").map(|m| ctx.text(m)).unwrap_or("");
            (last_path_seg(m) == "anyhow").then(|| "anyhow::Error".to_string())
        }
        _ => None,
    }
}

/// axum `get(list).post(create)` / actix `web::get().to(index)` → (METHOD, handler).
fn route_handlers<'t>(ctx: &Ctx, expr: Node<'t>) -> Vec<(String, Node<'t>)> {
    if expr.kind() != "call_expression" {
        return Vec::new();
    }
    let Some(f) = expr.child_by_field_name("function") else { return Vec::new() };
    let args = arg_nodes(expr.child_by_field_name("arguments"));
    let method_of = |m: &str| if m == "any" { "ANY".to_string() } else { m.to_ascii_uppercase() };
    match f.kind() {
        "identifier" | "scoped_identifier" => {
            let m = last_path_seg(ctx.text(f));
            if ROUTER_METHODS.contains(&m) && args.len() == 1 {
                vec![(method_of(m), args[0])]
            } else {
                Vec::new()
            }
        }
        "field_expression" => {
            let m = f.child_by_field_name("field").map(|x| ctx.text(x)).unwrap_or("");
            let Some(v) = f.child_by_field_name("value") else { return Vec::new() };
            let mut out = route_handlers(ctx, v);
            if ROUTER_METHODS.contains(&m) && args.len() == 1 {
                out.push((method_of(m), args[0]));
            } else if m == "to" && args.len() == 1 {
                let method = v
                    .child_by_field_name("function")
                    .map(|vf| last_path_seg(ctx.text(vf)).to_string())
                    .filter(|vm| ROUTER_METHODS.contains(&vm.as_str()))
                    .map(|vm| method_of(&vm))
                    .unwrap_or_else(|| "ANY".into());
                out.push((method, args[0]));
            }
            out
        }
        _ => Vec::new(),
    }
}

fn mark_entry(ctx: &mut Ctx, kind: &str) {
    let enclosing = ctx.enclosing();
    if let Some(s) = ctx.symbol_mut(&enclosing) {
        if !s.is_entrypoint {
            s.is_entrypoint = true;
            s.entry_kind = Some(kind.into());
        }
    }
}

fn call<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>) {
    let Some(mut func) = node.child_by_field_name("function") else { return };
    let mut generics = None;
    if func.kind() == "generic_function" {
        generics = func.child_by_field_name("type_arguments").map(|t| collapse_ws(ctx.text(t)));
        match func.child_by_field_name("function") {
            Some(f) => func = f,
            None => return,
        }
    }
    let args_node = node.child_by_field_name("arguments");
    let args = arg_nodes(args_node);
    let argc = Some(args.len() as u32);
    let line = line_of(node);
    let first_str = args.first().and_then(|a| string_arg(ctx, *a));
    match func.kind() {
        "identifier" => {
            let name = ctx.text(func).to_string();
            if name == "Err" {
                if let Some(e) = args.first().and_then(|a| error_type(ctx, st, *a)) {
                    let msg = args.first().and_then(|a| first_string_deep(ctx, *a));
                    ctx.push_ref(RefKind::Throw, &e, None, None, None, line, msg);
                }
            }
            if !ctx.is_local(&name) && !matches!(name.as_str(), "Some" | "Ok" | "Err" | "Box" | "drop") {
                ctx.push_ref(RefKind::Call, &name, None, None, argc, line, generics);
            }
            if matches!(name.as_str(), "insert_into" | "update" | "delete") {
                diesel_write(ctx, args.first().copied(), line);
            }
        }
        "scoped_identifier" => {
            let path = func.child_by_field_name("path").map(|p| clean_path(ctx.text(p))).unwrap_or_default();
            let Some(name) = func.child_by_field_name("name").map(|n| ctx.text(n).to_string()) else { return };
            let head = last_path_seg(&path);
            let rtype = if path == "Self" { st.self_type.clone() } else { None };
            // ---- Configuration ----
            if (head == "env" && matches!(name.as_str(), "var" | "var_os")) || (matches!(head, "dotenvy" | "dotenv") && name == "var") {
                if let Some(k) = first_str.clone() {
                    ctx.push_config(&k, "env", line, None);
                }
            }
            // ---- HTTP clients ----
            if matches!(head, "reqwest" | "ureq" | "surf" | "isahc" | "attohttpc")
                && matches!(name.as_str(), "get" | "post" | "put" | "delete" | "patch" | "head")
            {
                if let Some(url) = first_str.clone().filter(|u| u.contains('/')) {
                    ctx.push_http_call(&name, &url, head, line);
                }
            }
            // ---- Database (diesel DSL) ----
            if head == "diesel" && matches!(name.as_str(), "insert_into" | "update" | "delete" | "replace_into") {
                diesel_write(ctx, args.first().copied(), line);
            }
            // ---- Server start ----
            if (head == "axum" && name == "serve") || (head == "Server" && name == "bind") || (head == "warp" && name == "serve") {
                mark_entry(ctx, "server start");
            }
            ctx.push_ref(RefKind::Call, &name, path_receiver(&path), rtype, argc, line, generics);
        }
        "field_expression" => {
            let Some(value) = func.child_by_field_name("value") else { return };
            let name = func.child_by_field_name("field").map(|f| ctx.text(f).to_string()).unwrap_or_default();
            let (recv, rtype) = receiver_info(ctx, st, value);
            let recv_lower = recv.to_ascii_lowercase();
            let rtype_lower = rtype.as_deref().unwrap_or("").to_ascii_lowercase();
            // ---- Routes (axum `.route("/x", get(h))`, actix `.route("/x", web::get().to(h))`) ----
            if name == "route" && args.len() >= 2 {
                if let Some(path) = first_str.clone().filter(|p| p.starts_with('/')) {
                    let fw = st.web.unwrap_or(if recv_lower.contains("app::new") { "actix" } else { "axum" });
                    let enclosing = ctx.enclosing();
                    for (method, h) in route_handlers(ctx, args[1]) {
                        if matches!(h.kind(), "identifier" | "scoped_identifier") {
                            let target = last_path_seg(&clean_path(ctx.text(h))).to_string();
                            ctx.push_route(&format!("ref:{target}|{enclosing}"), &method, &path, fw, line);
                        }
                    }
                }
            }
            // ---- HTTP clients ----
            if matches!(name.as_str(), "get" | "post" | "put" | "delete" | "patch" | "head")
                && (recv_lower.ends_with("client")
                    || recv_lower.contains("reqwest")
                    || recv_lower.contains("http")
                    || rtype_lower.contains("client"))
            {
                if let Some(url) = first_str.clone().filter(|u| u.contains('/')) {
                    ctx.push_http_call(&name, &url, &recv, line);
                }
            }
            // ---- Configuration (`config` crate / settings objects) ----
            if matches!(name.as_str(), "get" | "get_string" | "get_int" | "get_bool" | "get_float" | "get_array" | "get_table")
                && (recv_lower.ends_with("config") || recv_lower.ends_with("settings") || rtype_lower.contains("config"))
            {
                if let Some(k) = first_str.clone() {
                    ctx.push_config(&k, "config", line, None);
                }
            }
            // ---- Server start ----
            if (name == "run" && recv_lower.contains("httpserver")) || (name == "launch" && recv_lower.contains("rocket")) {
                mark_entry(ctx, "server start");
            }
            // ---- Mutation of module statics: `CACHE.lock().unwrap().insert(…)` ----
            if matches!(
                name.as_str(),
                "push"
                    | "insert"
                    | "extend"
                    | "clear"
                    | "remove"
                    | "store"
                    | "fetch_add"
                    | "fetch_sub"
                    | "swap"
                    | "set"
                    | "push_back"
                    | "append"
                    | "retain"
                    | "truncate"
                    | "drain"
            ) {
                let head = recv.split(['.', '(', '[']).next().unwrap_or("").to_string();
                if !head.is_empty() && !ctx.is_local(&head) && ctx.is_module_var(&head) && ctx.in_function() {
                    ctx.push_ref(RefKind::Write, &head, None, None, None, line, None);
                    ctx.push_mutation(&head, "global_var", line);
                }
            }
            ctx.push_ref(RefKind::Call, &name, Some(recv), rtype, argc, line, generics);
            walk(ctx, st, value);
        }
        _ => walk(ctx, st, func),
    }
    for a in args {
        walk(ctx, st, a);
    }
}

/// First string literal inside an expression (error messages).
fn first_string_deep(ctx: &Ctx, n: Node) -> Option<String> {
    let mut stack = vec![n];
    while let Some(x) = stack.pop() {
        match x.kind() {
            "string_literal" | "raw_string_literal" => return Some(string_value(x, ctx.src)),
            "token_tree" => {
                if let Some(s) = first_string_in_tt(ctx, x) {
                    return Some(s);
                }
            }
            _ => {
                let mut kids = named_children(x);
                kids.reverse();
                stack.extend(kids);
            }
        }
    }
    None
}

/// diesel `insert_into(users::table)` / `update(users.find(id))` → write to table `users`.
fn diesel_write(ctx: &mut Ctx, arg: Option<Node>, line: u32) {
    let Some(a) = arg else { return };
    let text = ctx.text(a).trim_start_matches('&').to_string();
    if !(text.contains("::table") || text.ends_with("table") || text.contains(".filter(") || text.contains(".find(")) {
        return;
    }
    let table: String = text.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if !table.is_empty() {
        ctx.push_mutation(&format!("table:{table}"), "database", line);
    }
}

fn macro_call<'t>(ctx: &mut Ctx, st: &mut State<'t>, node: Node<'t>) {
    let Some(m) = node.child_by_field_name("macro") else { return };
    let full = clean_path(ctx.text(m));
    let name = last_path_seg(&full).to_string();
    let prefix = full.strip_suffix(&name).unwrap_or("").trim_end_matches(':').to_string();
    let line = line_of(node);
    let tt = token_tree_of(node);
    match name.as_str() {
        "panic" | "unreachable" | "todo" | "unimplemented" => {
            let msg = tt.and_then(|t| first_string_in_tt(ctx, t));
            ctx.push_ref(RefKind::Throw, "panic", None, None, None, line, msg);
        }
        "bail" => {
            let msg = tt.and_then(|t| first_string_in_tt(ctx, t));
            ctx.push_ref(RefKind::Throw, "anyhow::Error", None, None, None, line, msg);
        }
        "env" | "option_env" => {
            if let Some(k) = tt.and_then(|t| first_string_in_tt(ctx, t)) {
                ctx.push_config(&k, "env!", line, None);
            }
        }
        n if LOG_MACROS.contains(&n) && (prefix.is_empty() || matches!(prefix.as_str(), "log" | "tracing")) => {
            let id = ctx.enclosing();
            if let Some(s) = ctx.symbol_mut(&id) {
                s.capabilities.insert("LOGGING".into());
            }
        }
        n if !STD_MACROS.contains(&n) => {
            ctx.push_ref(RefKind::Call, &name, None, None, None, line, Some("macro".into()));
        }
        _ => {}
    }
    if OPAQUE_MACROS.contains(&name.as_str()) {
        return;
    }
    let Some(tt) = tt else { return };
    let Some(root) = st.macro_roots.get(st.level).copied() else { return };
    let args = macro_args(root, tt.start_byte(), tt.end_byte());
    let parsed = !args.is_empty() && !args.iter().any(|a| a.has_error());
    st.level += 1;
    for a in args {
        walk(ctx, st, a);
    }
    st.level -= 1;
    if !parsed {
        // Not expression syntax (`json!({ "k": f() })`, DSLs): at least the calls count.
        token_calls(ctx, vec![tt]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_types() {
        assert_eq!(clean_path("Vec::<T>"), "Vec");
        assert_eq!(clean_path("<T as Repo>"), "Repo");
        assert_eq!(clean_path("crate :: db"), "crate::db");
        assert_eq!(subst_self("Result<Self, Error>", Some("Foo")), "Result<Foo, Error>");
        assert_eq!(subst_self("SelfRef", Some("Foo")), "SelfRef");
        assert_eq!(unwrap_result_type("Result<Order, AppError>"), "Order");
        assert_eq!(unwrap_result_type("&Option<Repo>"), "Repo");
        assert_eq!(unwrap_type("@mt#Repo#find".into()), "@mt#Repo#find!await");
        assert_eq!(clean_type("&'a mut dyn Repo + Send"), "Repo");
        assert_eq!(clean_type("Arc<Mutex<OrderRepo>>"), "OrderRepo");
        assert_eq!(element_type("&[Order]").as_deref(), Some("Order"));
        assert_eq!(element_type("Vec<Order>").as_deref(), Some("Order"));
        let v = |xs: &[&str]| xs.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(relative_to_file(v(&["super"]), &v(&["tests"])), v(&["self"]));
        assert_eq!(relative_to_file(v(&["super", "super", "x"]), &v(&["tests"])), v(&["super", "x"]));
        assert_eq!(relative_to_file(v(&["self", "inner"]), &v(&["outer"])), v(&["self", "outer", "inner"]));
        assert_eq!(relative_to_file(v(&["super", "y"]), &v(&["a", "b"])), v(&["self", "a", "y"]));
        assert_eq!(clean_type("'asc' | 'desc'"), "'asc'", "TS string literal types are not lifetimes");
    }
}
