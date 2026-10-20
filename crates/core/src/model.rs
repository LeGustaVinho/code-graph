//! Syntactic facts extracted from a single file.
//!
//! Everything here is produced purely from the file's own AST: no cross-file
//! resolution happens in `codegraph-core`. The semantic crate turns these facts
//! into graph edges with explicit confidence levels.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::lang::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Module,
    Class,
    Interface,
    Struct,
    Enum,
    TypeAlias,
    Method,
    Constructor,
    Function,
    Property,
    Field,
    Variable,
    Const,
    Test,
    DataModel,
    Literal,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Module => "module",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::TypeAlias => "type_alias",
            SymbolKind::Method => "method",
            SymbolKind::Constructor => "constructor",
            SymbolKind::Function => "function",
            SymbolKind::Property => "property",
            SymbolKind::Field => "field",
            SymbolKind::Variable => "variable",
            SymbolKind::Const => "const",
            SymbolKind::Test => "test",
            SymbolKind::DataModel => "data_model",
            SymbolKind::Literal => "literal",
        }
    }

    pub fn parse(s: &str) -> Option<SymbolKind> {
        Some(match s {
            "module" => SymbolKind::Module,
            "class" => SymbolKind::Class,
            "interface" => SymbolKind::Interface,
            "struct" => SymbolKind::Struct,
            "enum" => SymbolKind::Enum,
            "type_alias" => SymbolKind::TypeAlias,
            "method" => SymbolKind::Method,
            "constructor" => SymbolKind::Constructor,
            "function" => SymbolKind::Function,
            "property" => SymbolKind::Property,
            "field" => SymbolKind::Field,
            "variable" => SymbolKind::Variable,
            "const" => SymbolKind::Const,
            "test" => SymbolKind::Test,
            "data_model" => SymbolKind::DataModel,
            "literal" => SymbolKind::Literal,
            _ => return None,
        })
    }

    /// Kinds that define a type (members can be looked up inside them).
    pub fn is_type(self) -> bool {
        matches!(
            self,
            SymbolKind::Class
                | SymbolKind::Interface
                | SymbolKind::Struct
                | SymbolKind::Enum
                | SymbolKind::DataModel
                | SymbolKind::TypeAlias
        )
    }

    /// Kinds that contain executable code.
    pub fn is_callable(self) -> bool {
        matches!(self, SymbolKind::Method | SymbolKind::Constructor | SymbolKind::Function | SymbolKind::Test)
    }

    /// Kinds that hold state (targets of READS_VAR / WRITES_VAR).
    pub fn is_variable(self) -> bool {
        matches!(self, SymbolKind::Variable | SymbolKind::Const | SymbolKind::Field | SymbolKind::Property)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Param {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub variadic: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ParamInfo {
    pub total: u32,
    pub required: u32,
    pub variadic: bool,
    pub list: Vec<Param>,
}

impl ParamInfo {
    pub fn from_list(list: Vec<Param>) -> ParamInfo {
        let total = list.iter().filter(|p| !p.variadic).count() as u32;
        let required = list.iter().filter(|p| !p.optional && !p.variadic).count() as u32;
        let variadic = list.iter().any(|p| p.variadic);
        ParamInfo { total, required, variadic, list }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Symbol {
    pub id: String,
    pub kind: SymbolKind,
    pub name: String,
    pub qualified_name: String,
    pub signature: String,
    pub start_line: u32,
    pub end_line: u32,
    pub parent_id: Option<String>,
    pub visibility: String,
    pub is_exported: bool,
    pub is_static: bool,
    pub is_async: bool,
    pub is_abstract: bool,
    pub capabilities: BTreeSet<String>,
    pub complexity: u32,
    pub is_entrypoint: bool,
    pub entry_kind: Option<String>,
    pub is_test: bool,
    pub docstring: Option<String>,
    /// Attribute / decorator expressions, compacted (e.g. `HttpGet("{id}")`).
    pub attrs: Vec<String>,
    /// Declared base types / implemented interfaces (raw names).
    pub bases: Vec<String>,
    /// Declared type for fields/variables/properties, return type for callables.
    pub type_ref: Option<String>,
    /// Initializer text (truncated) for variables, constants and fields.
    pub value_text: Option<String>,
    pub params: Option<ParamInfo>,
    /// Hash of the symbol's token stream without comments/whitespace.
    pub body_hash: String,
    pub namespace: Option<String>,
    /// Declaration modifiers that change resolution (`virtual`, `override`, `sealed`, `new`,
    /// `partial`, `extern`, `required`, `file`, `readonly`, `explicit:IFoo`, `extension`).
    #[serde(default)]
    pub modifiers: Vec<String>,
}

impl Symbol {
    pub fn coords(&self, file: &str) -> String {
        format!("{}:{}-{}", file, self.start_line, self.end_line)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// Function / method invocation.
    Call,
    /// Object construction (`new T()` / `T()` for Python classes).
    New,
    /// JSX element usage (`<Component />`).
    Jsx,
    Throw,
    /// `catch (T)` / `except T` site (name = exception type, `*` for catch-all).
    Catch,
    Read,
    Write,
    /// A type named in code without calling it: `typeof(T)`, attributes `[T]`, generic
    /// arguments of `GetComponent<T>()`, type annotations.
    TypeUse,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub kind: RefKind,
    /// Id of the innermost enclosing symbol (module node for top-level code).
    pub enclosing: String,
    pub name: String,
    /// Simplified receiver expression: `this`, `self`, `this._svc`, `axios`, `Foo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver: Option<String>,
    /// Statically known type of the receiver (from declarations in scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
    /// For member chains (`a.b.c`), the declared type of the head `a` (or the enclosing class for `this.`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_count: Option<u32>,
    pub line: u32,
    /// Exception types caught by `try` blocks enclosing this site in the same function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catch_types: Vec<String>,
    /// Message literal (throws) or generic type arguments (calls).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    Named,
    Default,
    Namespace,
    SideEffect,
    Using,
    UsingStatic,
    /// C# 10 `global using X;`: visible from every file of the project.
    GlobalUsing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Import {
    pub module: String,
    /// Imported symbol name (None for namespace / default / side-effect imports).
    pub imported: Option<String>,
    /// Local binding name.
    pub local: Option<String>,
    pub kind: ImportKind,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Literal {
    pub value: String,
    pub literal_type: String,
    pub line: u32,
    pub column: u32,
    pub enclosing: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigDep {
    pub key: String,
    pub source_type: String,
    pub line: u32,
    pub default_value: Option<String>,
    /// `read` for code that consumes the key, `define` for config files that declare it.
    pub usage: String,
    pub enclosing: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mutation {
    pub enclosing: String,
    pub target: String,
    pub mutation_type: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DiRegistration {
    /// Service type / injection token.
    pub service: String,
    pub implementation: Option<String>,
    pub lifetime: String,
    pub framework: String,
    pub line: u32,
    pub enclosing: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouteDecl {
    pub handler: String,
    pub http_method: String,
    pub path: String,
    pub framework: String,
    pub line: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HttpCall {
    pub enclosing: String,
    pub http_method: String,
    pub url: String,
    pub client: String,
    pub line: u32,
}

/// Framework-level exception handler (middleware, exception filters, error handlers).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GlobalHandler {
    pub handler: String,
    pub exception: String,
    pub framework: String,
    pub line: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub path: String,
    pub language: Option<Language>,
    pub line_count: u32,
    pub namespace: Option<String>,
    pub symbols: Vec<Symbol>,
    pub refs: Vec<Reference>,
    pub imports: Vec<Import>,
    pub literals: Vec<Literal>,
    pub config: Vec<ConfigDep>,
    pub mutations: Vec<Mutation>,
    pub di: Vec<DiRegistration>,
    pub routes: Vec<RouteDecl>,
    pub http_calls: Vec<HttpCall>,
    pub global_handlers: Vec<GlobalHandler>,
    /// Project-wide route prefixes declared in this file (e.g. NestJS `setGlobalPrefix`).
    pub route_prefixes: Vec<String>,
    /// Syntax error / missing nodes Tree-sitter recovered from (0 = clean parse).
    #[serde(default)]
    pub syntax_errors: u32,
}

impl FileFacts {
    pub fn module_id(&self) -> &str {
        self.symbols.first().map(|s| s.id.as_str()).unwrap_or("")
    }

    pub fn symbol(&self, id: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.id == id)
    }
}
