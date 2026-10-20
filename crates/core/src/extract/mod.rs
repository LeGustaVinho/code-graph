//! Syntactic fact extraction (Tree-sitter AST → [`FileFacts`]).

mod capabilities;
pub mod config_files;
pub mod csharp;
pub mod prisma;
pub mod python;
pub mod typescript;

use std::collections::{BTreeSet, HashMap};
use tree_sitter::Node;

use crate::hash::symbol_id;
use crate::lang::{parse, Language};
use crate::model::*;

/// Extracts all syntactic facts from one file. Never panics on malformed input:
/// Tree-sitter is error tolerant and every extractor works on partial trees.
pub fn extract_file(path: &str, lang: Language, source: &str) -> FileFacts {
    let line_count = source.lines().count().max(1) as u32;
    let neutralized;
    let source = if lang == Language::CSharp && source.contains('#') {
        neutralized = neutralize_csharp_preprocessor(source);
        neutralized.as_str()
    } else {
        source
    };
    let mut ctx = Ctx::new(path, lang, source, line_count);
    match lang {
        Language::Prisma => prisma::extract(&mut ctx),
        Language::Config => config_files::extract(&mut ctx),
        _ => {
            if let Some(tree) = parse(lang, source) {
                let root = tree.root_node();
                ctx.facts.symbols[0].body_hash = token_hash(root, source);
                match lang {
                    Language::CSharp => csharp::extract(&mut ctx, root),
                    Language::Python => python::extract(&mut ctx, root),
                    _ => typescript::extract(&mut ctx, root),
                }
            }
        }
    }
    ctx.finish()
}

#[derive(Default, Clone)]
pub(crate) struct Scope {
    pub symbol_id: String,
    pub kind: Option<SymbolKind>,
    pub qname: String,
    /// Local variables / parameters → declared or inferred type.
    pub locals: HashMap<String, Option<String>>,
    /// Class fields → declared type (only for type scopes).
    pub fields: HashMap<String, Option<String>>,
    /// Stack of catch-type lists for enclosing `try` blocks (function scopes only).
    pub catch_stack: Vec<Vec<String>>,
    pub is_function: bool,
    /// Python `global x` declarations.
    pub globals: BTreeSet<String>,
    /// Route-group prefixes bound to local variables (`var g = app.MapGroup("/api")`).
    pub route_groups: HashMap<String, String>,
}

pub(crate) struct Ctx<'a> {
    pub src: &'a str,
    pub path: &'a str,
    pub lang: Language,
    pub facts: FileFacts,
    pub scopes: Vec<Scope>,
    qname_counts: HashMap<(SymbolKind, String), u32>,
    /// Symbols (by id) that contain an `await`.
    pub awaits: BTreeSet<String>,
    /// Module path used as the prefix of qualified names (python dotted path).
    #[allow(dead_code)]
    pub module_qname: String,
}

pub(crate) struct NewSymbol {
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub signature: String,
    pub start_line: u32,
    pub end_line: u32,
    pub visibility: String,
    pub is_exported: bool,
    pub is_static: bool,
    pub is_async: bool,
    pub is_abstract: bool,
    pub attrs: Vec<String>,
    pub bases: Vec<String>,
    pub type_ref: Option<String>,
    pub value_text: Option<String>,
    pub params: Option<ParamInfo>,
    pub docstring: Option<String>,
    pub body_hash: String,
    pub complexity: u32,
    pub is_test: bool,
    pub is_entrypoint: bool,
    pub entry_kind: Option<String>,
}

impl NewSymbol {
    pub fn new(kind: SymbolKind, name: &str, qualified_name: String, start_line: u32, end_line: u32) -> Self {
        NewSymbol {
            kind,
            name: name.to_string(),
            qualified_name,
            signature: String::new(),
            start_line,
            end_line,
            visibility: "public".into(),
            is_exported: false,
            is_static: false,
            is_async: false,
            is_abstract: false,
            attrs: Vec::new(),
            bases: Vec::new(),
            type_ref: None,
            value_text: None,
            params: None,
            docstring: None,
            body_hash: String::new(),
            complexity: 0,
            is_test: false,
            is_entrypoint: false,
            entry_kind: None,
        }
    }
}

impl<'a> Ctx<'a> {
    fn new(path: &'a str, lang: Language, src: &'a str, line_count: u32) -> Self {
        let module_qname = module_qname_for(path, lang);
        let stem = file_stem(path);
        let module_id = symbol_id(path, "module", &module_qname, 0);
        let module = Symbol {
            id: module_id.clone(),
            kind: SymbolKind::Module,
            name: stem,
            qualified_name: module_qname.clone(),
            signature: path.to_string(),
            start_line: 1,
            end_line: line_count,
            parent_id: None,
            visibility: "public".into(),
            is_exported: true,
            is_static: false,
            is_async: false,
            is_abstract: false,
            capabilities: BTreeSet::new(),
            complexity: 0,
            is_entrypoint: false,
            entry_kind: None,
            is_test: is_test_path(path),
            docstring: None,
            attrs: Vec::new(),
            bases: Vec::new(),
            type_ref: None,
            value_text: None,
            params: None,
            body_hash: String::new(),
            namespace: None,
        };
        let facts = FileFacts { path: path.to_string(), language: Some(lang), line_count, symbols: vec![module], ..Default::default() };
        let scope =
            Scope { symbol_id: module_id, kind: Some(SymbolKind::Module), qname: String::new(), is_function: false, ..Default::default() };
        Ctx { src, path, lang, facts, scopes: vec![scope], qname_counts: HashMap::new(), awaits: BTreeSet::new(), module_qname }
    }

    pub fn text(&self, node: Node) -> &'a str {
        &self.src[node.byte_range()]
    }

    pub fn module_id(&self) -> String {
        self.facts.symbols[0].id.clone()
    }

    /// Qualified name for a new child declared in the current scope.
    pub fn child_qname(&self, name: &str) -> String {
        let parent = self.scopes.last().map(|s| s.qname.as_str()).unwrap_or("");
        if parent.is_empty() {
            name.to_string()
        } else {
            format!("{parent}.{name}")
        }
    }

    /// Registers a symbol under the innermost scope and returns its id.
    pub fn add_symbol(&mut self, s: NewSymbol) -> String {
        let key = (s.kind, s.qualified_name.clone());
        let n = self.qname_counts.entry(key).or_insert(0);
        let id = symbol_id(self.path, s.kind.as_str(), &s.qualified_name, *n);
        *n += 1;
        let parent_id = self.scopes.last().map(|sc| sc.symbol_id.clone());
        let namespace = self.facts.namespace.clone();
        self.facts.symbols.push(Symbol {
            id: id.clone(),
            kind: s.kind,
            name: s.name,
            qualified_name: s.qualified_name,
            signature: truncate(&collapse_ws(&s.signature), 240),
            start_line: s.start_line,
            end_line: s.end_line.max(s.start_line),
            parent_id,
            visibility: s.visibility,
            is_exported: s.is_exported,
            is_static: s.is_static,
            is_async: s.is_async,
            is_abstract: s.is_abstract,
            capabilities: BTreeSet::new(),
            complexity: s.complexity,
            is_entrypoint: s.is_entrypoint,
            entry_kind: s.entry_kind,
            is_test: s.is_test,
            docstring: s.docstring,
            attrs: s.attrs,
            bases: s.bases,
            type_ref: s.type_ref,
            value_text: s.value_text.map(|v| truncate(&collapse_ws(&v), 160)),
            params: s.params,
            body_hash: s.body_hash,
            namespace: if self.lang == Language::CSharp { namespace } else { None },
        });
        id
    }

    pub fn symbol_mut(&mut self, id: &str) -> Option<&mut Symbol> {
        self.facts.symbols.iter_mut().rev().find(|s| s.id == id)
    }

    pub fn push_scope(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Id of the innermost enclosing symbol (module for top-level code).
    pub fn enclosing(&self) -> String {
        self.scopes.last().map(|s| s.symbol_id.clone()).unwrap_or_default()
    }

    pub fn in_function(&self) -> bool {
        self.scopes.last().map(|s| s.is_function).unwrap_or(false)
    }

    /// Declares a local variable in the innermost function scope (or module scope).
    pub fn declare_local(&mut self, name: &str, ty: Option<String>) {
        if let Some(s) = self.scopes.last_mut() {
            s.locals.insert(name.to_string(), ty);
        }
    }

    /// True if `name` is a local/parameter of an enclosing function scope.
    pub fn is_local(&self, name: &str) -> bool {
        for s in self.scopes.iter().rev() {
            if s.kind == Some(SymbolKind::Module) {
                return false;
            }
            if s.is_function && s.locals.contains_key(name) {
                return true;
            }
        }
        false
    }

    pub fn is_module_var(&self, name: &str) -> bool {
        self.scopes.first().map(|s| s.locals.contains_key(name)).unwrap_or(false)
    }

    /// Looks up the static type of a variable: locals → class fields → module vars.
    pub fn var_type(&self, name: &str) -> Option<String> {
        for s in self.scopes.iter().rev() {
            if let Some(t) = s.locals.get(name) {
                return t.clone();
            }
            if let Some(t) = s.fields.get(name) {
                return t.clone();
            }
        }
        None
    }

    /// Type of a field of the nearest enclosing type.
    pub fn field_type(&self, name: &str) -> Option<String> {
        for s in self.scopes.iter().rev() {
            if s.kind.map(|k| k.is_type()).unwrap_or(false) {
                return s.fields.get(name).cloned().flatten();
            }
        }
        None
    }

    pub fn is_field(&self, name: &str) -> bool {
        for s in self.scopes.iter().rev() {
            if s.kind.map(|k| k.is_type()).unwrap_or(false) {
                return s.fields.contains_key(name);
            }
        }
        false
    }

    /// Qualified name of the nearest enclosing type.
    pub fn class_qname(&self) -> Option<String> {
        self.scopes.iter().rev().find(|s| s.kind.map(|k| k.is_type()).unwrap_or(false)).map(|s| s.qname.clone())
    }

    pub fn class_name(&self) -> Option<String> {
        self.class_qname().map(|q| q.rsplit('.').next().unwrap_or(&q).to_string())
    }

    pub fn current_catch_types(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in self.scopes.iter().rev() {
            for frame in s.catch_stack.iter().rev() {
                for t in frame {
                    if !out.contains(t) {
                        out.push(t.clone());
                    }
                }
            }
            if s.is_function {
                break;
            }
        }
        out
    }

    pub fn push_catch_frame(&mut self, types: Vec<String>) {
        if let Some(s) = self.scopes.last_mut() {
            s.catch_stack.push(types);
        }
    }

    pub fn pop_catch_frame(&mut self) {
        if let Some(s) = self.scopes.last_mut() {
            s.catch_stack.pop();
        }
    }

    pub fn mark_await(&mut self) {
        let id = self.enclosing();
        self.awaits.insert(id);
    }

    pub fn push_ref(
        &mut self,
        kind: RefKind,
        name: &str,
        receiver: Option<String>,
        receiver_type: Option<String>,
        arg_count: Option<u32>,
        line: u32,
        detail: Option<String>,
    ) {
        if name.is_empty() {
            return;
        }
        let catch_types = match kind {
            RefKind::Call | RefKind::New | RefKind::Throw => self.current_catch_types(),
            _ => Vec::new(),
        };
        let enclosing = self.enclosing();
        if matches!(kind, RefKind::Read) {
            // Reads are deduplicated per (enclosing, name, receiver) to keep the fact set small.
            if self
                .facts
                .refs
                .iter()
                .rev()
                .take(64)
                .any(|r| r.kind == RefKind::Read && r.enclosing == enclosing && r.name == name && r.receiver == receiver)
            {
                return;
            }
        }
        let receiver_type = receiver_type.map(|t| clean_type(&t)).filter(|t| !t.is_empty());
        let root_type = match (&receiver, &receiver_type) {
            (Some(r), None) if r.contains('.') => {
                let head = r.split(['.', '?', '[', '(', '!']).next().unwrap_or("").trim();
                let t = if head == "this" || head == "self" {
                    self.class_name()
                } else if self.is_local(head) || self.is_field(head) || self.is_module_var(head) {
                    self.var_type(head)
                } else {
                    None
                };
                t.map(|t| clean_type(&t)).filter(|t| !t.is_empty())
            }
            _ => None,
        };
        self.facts.refs.push(Reference {
            kind,
            enclosing,
            name: name.to_string(),
            receiver,
            receiver_type,
            root_type,
            arg_count,
            line,
            catch_types,
            detail,
        });
    }

    pub fn push_literal(&mut self, value: &str, literal_type: &str, node: Node) {
        let value = truncate(value, 300);
        if value.is_empty() {
            return;
        }
        if literal_type == "number" && matches!(value.as_str(), "0" | "1" | "-1") {
            return;
        }
        let line = line_of(node);
        if literal_type != "number" {
            if let Some(table) = sql_write_table(&value) {
                let enclosing = self.enclosing();
                self.facts.mutations.push(Mutation { enclosing, target: table, mutation_type: "database".into(), line });
            }
        }
        self.facts.literals.push(Literal {
            value,
            literal_type: literal_type.to_string(),
            line,
            column: node.start_position().column as u32 + 1,
            enclosing: self.enclosing(),
        });
    }

    pub fn push_mutation(&mut self, target: &str, mutation_type: &str, line: u32) {
        let enclosing = self.enclosing();
        if self.facts.mutations.iter().any(|m| m.enclosing == enclosing && m.target == target && m.line == line) {
            return;
        }
        self.facts.mutations.push(Mutation { enclosing, target: target.to_string(), mutation_type: mutation_type.to_string(), line });
    }

    pub fn push_config(&mut self, key: &str, source_type: &str, line: u32, default_value: Option<String>) {
        if key.is_empty() {
            return;
        }
        let enclosing = Some(self.enclosing());
        self.facts.config.push(ConfigDep {
            key: key.to_string(),
            source_type: source_type.to_string(),
            line,
            default_value,
            usage: "read".into(),
            enclosing,
        });
    }

    pub fn push_http_call(&mut self, method: &str, url: &str, client: &str, line: u32) {
        let enclosing = self.enclosing();
        self.facts.http_calls.push(HttpCall {
            enclosing,
            http_method: method.to_ascii_uppercase(),
            url: url.to_string(),
            client: client.to_string(),
            line,
        });
    }

    pub fn push_route(&mut self, handler: &str, method: &str, path: &str, framework: &str, line: u32) {
        self.facts.routes.push(RouteDecl {
            handler: handler.to_string(),
            http_method: method.to_ascii_uppercase(),
            path: path.to_string(),
            framework: framework.to_string(),
            line,
        });
    }

    pub fn push_di(&mut self, service: &str, implementation: Option<&str>, lifetime: &str, framework: &str, line: u32) {
        let enclosing = self.enclosing();
        self.facts.di.push(DiRegistration {
            service: service.to_string(),
            implementation: implementation.map(|s| s.to_string()),
            lifetime: lifetime.to_string(),
            framework: framework.to_string(),
            line,
            enclosing,
        });
    }

    fn finish(mut self) -> FileFacts {
        capabilities::infer(&mut self);
        for id in &self.awaits {
            if let Some(s) = self.facts.symbols.iter_mut().find(|s| &s.id == id) {
                s.capabilities.insert("ASYNC".into());
            }
        }
        // A type that declares test methods is a test fixture (xUnit has no class attribute).
        let test_parents: std::collections::HashSet<String> =
            self.facts.symbols.iter().filter(|s| s.is_test && s.kind.is_callable()).filter_map(|s| s.parent_id.clone()).collect();
        for s in self.facts.symbols.iter_mut() {
            if s.is_async {
                s.capabilities.insert("ASYNC".into());
            }
            if s.kind.is_type() && test_parents.contains(&s.id) {
                s.is_test = true;
            }
        }
        self.facts
    }
}

// ---------------------------------------------------------------------------
// Generic helpers
// ---------------------------------------------------------------------------

pub fn line_of(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

pub fn end_line_of(node: Node) -> u32 {
    let end = node.end_position();
    // A node ending at column 0 actually ends on the previous line.
    if end.column == 0 && end.row > node.start_position().row {
        end.row as u32
    } else {
        end.row as u32 + 1
    }
}

pub fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Removes generic arguments, nullability, array markers and namespaces: `List<Order>?` → `List`.
pub fn clean_type(t: &str) -> String {
    let t = t.trim().trim_start_matches(':').trim();
    let t = t.trim_end_matches('?').trim_end_matches("[]");
    // Python Optional[T] / TS T | null.
    let t = if let Some(inner) = t.strip_prefix("Optional[").and_then(|x| x.strip_suffix(']')) { inner } else { t };
    let t = t.split('|').map(|p| p.trim()).find(|p| *p != "null" && *p != "undefined" && *p != "None").unwrap_or(t);
    let base = t.split(['<', '[', '(']).next().unwrap_or(t).trim();
    base.trim_start_matches("readonly ").trim().to_string()
}

/// Generic type arguments of a type expression: `Repository<Order>` → [`Order`].
pub fn generic_args(t: &str) -> Vec<String> {
    let Some(start) = t.find('<') else { return Vec::new() };
    let Some(end) = t.rfind('>') else { return Vec::new() };
    if end <= start {
        return Vec::new();
    }
    let inner = &t[start + 1..end];
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in inner.chars() {
        match c {
            '<' => {
                depth += 1;
                cur.push(c)
            }
            '>' => {
                depth -= 1;
                cur.push(c)
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

pub fn file_stem(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.find('.') {
        Some(0) | None => name.to_string(),
        Some(i) => name[..i].to_string(),
    }
}

/// Python dotted module path / extension-less path for other languages.
pub fn module_qname_for(path: &str, lang: Language) -> String {
    match lang {
        Language::Python => {
            let p = path.trim_end_matches(".pyi").trim_end_matches(".py");
            let p = p.trim_end_matches("/__init__");
            p.replace('/', ".")
        }
        _ => match path.rfind('.') {
            Some(i) if i > path.rfind('/').map(|x| x + 1).unwrap_or(0) => path[..i].to_string(),
            _ => path.to_string(),
        },
    }
}

pub fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    // C# test classes are `FooTests.cs` / `FooTest.cs`: the capital T marks a word boundary,
    // so `Latest.cs` or `Contest.cs` are not tests.
    let orig_name = path.rsplit('/').next().unwrap_or(path);
    let cs_test =
        orig_name.ends_with("Tests.cs") || orig_name.ends_with("Test.cs") || name.ends_with("_tests.cs") || name.ends_with("_test.cs");
    name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("-spec.")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || cs_test
        || lower.contains("/__tests__/")
        || lower.starts_with("__tests__/")
        || lower.contains(".tests/")
        || lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
}

/// Hash of the node's token stream excluding comments (formatting-insensitive).
pub fn token_hash(node: Node, src: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind().contains("comment") {
            continue;
        }
        if n.child_count() == 0 {
            hasher.update(src[n.byte_range()].as_bytes());
            hasher.update(b" ");
            continue;
        }
        let mut c = n.walk();
        let children: Vec<Node> = n.children(&mut c).collect();
        stack.extend(children.into_iter().rev());
    }
    hasher.finalize().to_hex()[..16].to_string()
}

/// Cyclomatic complexity: 1 + decision points, without descending into nested declarations.
pub fn cyclomatic(node: Node, src: &str, lang: Language) -> u32 {
    fn is_decision(n: Node, src: &str, lang: Language) -> bool {
        let k = n.kind();
        match lang {
            Language::CSharp => match k {
                "if_statement"
                | "while_statement"
                | "for_statement"
                | "foreach_statement"
                | "for_each_statement"
                | "do_statement"
                | "catch_clause"
                | "conditional_expression"
                | "switch_expression_arm" => true,
                "switch_section" => !src[n.byte_range()].trim_start().starts_with("default"),
                "binary_expression" => {
                    n.child_by_field_name("operator").map(|o| matches!(&src[o.byte_range()], "&&" | "||" | "??")).unwrap_or(false)
                }
                _ => false,
            },
            Language::Python => matches!(
                k,
                "if_statement"
                    | "elif_clause"
                    | "for_statement"
                    | "while_statement"
                    | "except_clause"
                    | "conditional_expression"
                    | "boolean_operator"
                    | "for_in_clause"
                    | "if_clause"
                    | "case_clause"
            ),
            _ => match k {
                "if_statement" | "for_statement" | "for_in_statement" | "while_statement" | "do_statement" | "switch_case"
                | "catch_clause" | "ternary_expression" => true,
                "binary_expression" => {
                    n.child_by_field_name("operator").map(|o| matches!(&src[o.byte_range()], "&&" | "||" | "??")).unwrap_or(false)
                }
                _ => false,
            },
        }
    }
    fn is_nested_decl(n: Node, lang: Language) -> bool {
        let k = n.kind();
        match lang {
            Language::CSharp => matches!(k, "class_declaration" | "struct_declaration" | "interface_declaration" | "record_declaration"),
            Language::Python => matches!(k, "class_definition" | "function_definition"),
            _ => matches!(k, "class_declaration" | "function_declaration" | "method_definition" | "class"),
        }
    }
    fn walk(n: Node, src: &str, lang: Language, root: bool, acc: &mut u32) {
        if !root && is_nested_decl(n, lang) {
            return;
        }
        if is_decision(n, src, lang) {
            *acc += 1;
        }
        let mut c = n.walk();
        for ch in n.named_children(&mut c) {
            walk(ch, src, lang, false, acc);
        }
    }
    let mut acc = 1;
    walk(node, src, lang, true, &mut acc);
    acc
}

/// Leading doc comment (`///`, `/** */`, `//`) directly above `node` (or its wrapper).
pub fn doc_comment(node: Node, src: &str) -> Option<String> {
    let mut target = node;
    if let Some(p) = node.parent() {
        if matches!(p.kind(), "export_statement" | "decorated_definition") {
            target = p;
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let mut cur = target.prev_sibling();
    let mut expected_row = target.start_position().row;
    while let Some(c) = cur {
        if !c.kind().contains("comment") {
            break;
        }
        if c.end_position().row + 1 < expected_row {
            break;
        }
        lines.insert(0, src[c.byte_range()].to_string());
        expected_row = c.start_position().row;
        cur = c.prev_sibling();
    }
    if lines.is_empty() {
        return None;
    }
    let mut text = String::new();
    for l in lines.join("\n").lines() {
        let l = l
            .trim()
            .trim_start_matches("///")
            .trim_start_matches("/**")
            .trim_start_matches("/*")
            .trim_start_matches("//")
            .trim_end_matches("*/")
            .trim_start_matches('*')
            .trim();
        let l = strip_xml_tags(l);
        if !l.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&l);
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(truncate(&text, 280))
    }
}

fn strip_xml_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Table written by a raw SQL DML statement, if `literal` is one.
pub fn sql_write_table(literal: &str) -> Option<String> {
    let t = literal.trim_start();
    let upper: String = t.chars().take(64).collect::<String>().to_ascii_uppercase();
    let rest = if let Some(r) = upper.strip_prefix("INSERT INTO ") {
        r
    } else if let Some(r) = upper.strip_prefix("UPDATE ") {
        r
    } else if let Some(r) = upper.strip_prefix("DELETE FROM ") {
        r
    } else {
        return None;
    };
    let offset = upper.len() - rest.len();
    let name: String = t[offset..]
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '"' || *c == '[' || *c == ']' || *c == '`')
        .collect();
    let name = name.trim_matches(|c| c == '"' || c == '[' || c == ']' || c == '`').to_string();
    if name.is_empty() {
        None
    } else {
        Some(format!("table:{name}"))
    }
}

/// Text of a string literal node without quotes (handles all three grammars).
pub fn string_value(node: Node, src: &str) -> String {
    let mut out = String::new();
    let mut c = node.walk();
    let mut found = false;
    for ch in node.named_children(&mut c) {
        match ch.kind() {
            "string_fragment"
            | "string_content"
            | "string_literal_content"
            | "escape_sequence"
            | "string_literal_encoding"
            | "raw_string_content"
            | "verbatim_string_literal_content" => {
                out.push_str(&src[ch.byte_range()]);
                found = true;
            }
            _ => {}
        }
    }
    if found {
        return out;
    }
    let raw = &src[node.byte_range()];
    let raw = raw.trim_start_matches('@').trim_start_matches('$');
    let raw = raw.trim_start_matches(|c: char| c.is_ascii_alphabetic() && c != '"' && c != '\'' && c != '`');
    raw.trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
}

pub fn first_string_arg<'t>(args: Node<'t>, src: &str) -> Option<(String, Node<'t>)> {
    let mut c = args.walk();
    let first = args.named_children(&mut c).next()?;
    let target = if first.kind() == "argument" { first.named_child(0)? } else { first };
    string_like(target, src).map(|s| (s, target))
}

/// Value of a string or template literal (templates keep `${...}` placeholders).
pub fn string_like(node: Node, src: &str) -> Option<String> {
    match node.kind() {
        "string" | "string_literal" | "verbatim_string_literal" | "raw_string_literal" => {
            if node.kind() == "string" && has_child_kind(node, "interpolation") {
                return Some(template_text(node, src));
            }
            Some(string_value(node, src))
        }
        "template_string" | "interpolated_string_expression" => Some(template_text(node, src)),
        _ => None,
    }
}

pub fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut c = node.walk();
    let r = node.named_children(&mut c).any(|ch| ch.kind() == kind);
    r
}

/// Template / interpolated string with substitutions normalized to `${expr}`.
pub fn template_text(node: Node, src: &str) -> String {
    let raw = &src[node.byte_range()];
    let raw = raw.trim_start_matches('$').trim_start_matches('@').trim_start_matches(['f', 'F', 'r', 'R', 'b', 'B']);
    let inner = raw.trim_matches(|c| c == '`' || c == '"' || c == '\'');
    let mut out = String::new();
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            out.push_str("${");
            chars.next();
            let mut depth = 1;
            for d in chars.by_ref() {
                if d == '{' {
                    depth += 1;
                }
                if d == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                out.push(d);
            }
            out.push('}');
        } else if c == '{' && chars.peek() != Some(&'{') {
            // C# / Python interpolation `{expr}`
            out.push_str("${");
            let mut depth = 1;
            for d in chars.by_ref() {
                if d == '{' {
                    depth += 1;
                }
                if d == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                out.push(d);
            }
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn count_args(args: Node) -> u32 {
    let mut c = args.walk();
    let n = args.named_children(&mut c).filter(|n| !n.kind().contains("comment")).count() as u32;
    n
}

/// Last identifier-ish segment of an expression text: `this.repo.save` → `save`.
pub fn last_segment(s: &str) -> &str {
    s.rsplit(['.', ':']).next().unwrap_or(s).trim_end_matches("?").trim()
}

pub fn simplify_receiver(text: &str) -> String {
    let t = collapse_ws(text).replace("?.", ".").replace("!.", ".");
    truncate(&t, 80)
}

pub fn is_upper_ident(s: &str) -> bool {
    s.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
}

pub fn is_const_name(s: &str) -> bool {
    s.len() > 1 && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') && s.chars().any(|c| c.is_ascii_uppercase())
}

/// Removes exactly one pair of surrounding parentheses: `(a, b=f())` → `a, b=f()`.
pub fn strip_parens(s: &str) -> &str {
    let t = s.trim();
    let t = t.strip_prefix('(').unwrap_or(t);
    t.strip_suffix(')').unwrap_or(t)
}

/// Makes C# with conditional compilation parseable: keeps the first branch of every
/// `#if` chain (as if its condition held), blanks `#elif` / `#else` branches and all
/// directive lines. Every removed byte becomes a space (newlines kept), so byte offsets
/// and line numbers are identical to the original file.
pub fn neutralize_csharp_preprocessor(src: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    // One flag per open `#if`: is the current branch of that level active?
    let mut stack: Vec<bool> = Vec::new();
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        let directive = t.starts_with('#');
        let blank = if directive {
            let word: String = t[1..].trim_start().chars().take_while(|c| c.is_ascii_alphabetic()).collect();
            match word.as_str() {
                "if" => stack.push(true),
                "elif" | "else" => {
                    if let Some(top) = stack.last_mut() {
                        *top = false;
                    }
                }
                "endif" => {
                    stack.pop();
                }
                _ => {}
            }
            true
        } else {
            stack.iter().any(|active| !active)
        };
        if blank {
            out.extend(line.bytes().map(|b| if b == b'\n' || b == b'\r' { b } else { b' ' }));
        } else {
            out.extend_from_slice(line.as_bytes());
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

#[cfg(test)]
mod preprocessor_tests {
    use super::*;

    #[test]
    fn keeps_first_branch_and_offsets() {
        let src = "class A {\n#if UNITY_EDITOR\n  void E() {}\n#else\n  void E() { broken(\n#endif\n  void F() {}\n}\n";
        let out = neutralize_csharp_preprocessor(src);
        assert_eq!(out.len(), src.len());
        assert_eq!(out.lines().count(), src.lines().count());
        assert!(out.contains("void E() {}") && !out.contains("broken") && !out.contains("#if"));
        let f = extract_file("A.cs", Language::CSharp, src);
        let names: Vec<&str> = f.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"E") && names.contains(&"F"), "{names:?}");
        let fsym = f.symbols.iter().find(|s| s.name == "F").unwrap();
        assert_eq!(fsym.start_line, 7);
    }
}
