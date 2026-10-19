//! The 28 analytical MCP tools.
//!
//! Every tool returns an [`Out`] (header lines + result items). The dispatcher applies
//! the optional `max_token_budget` uniformly, emitting an explicit
//! `[TRUNCATED: Showing X of Y results to fit N token budget (M tokens used)]` marker.

#[allow(dead_code)]
mod behavior;
#[allow(dead_code)]
mod data;
#[allow(dead_code)]
mod discovery;
pub mod fmt;
#[allow(dead_code)]
mod git;
#[allow(dead_code)]
mod governance;
#[allow(dead_code)]
mod navigation;
#[allow(dead_code)]
mod tokens;

use std::collections::HashMap;

use codegraph_core::tokens::BudgetWriter;
use codegraph_graph::query;
use codegraph_graph::Node;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::engine::Engine;

#[derive(Default)]
pub struct Out {
    pub header: Vec<String>,
    pub items: Vec<String>,
    /// Results that exist but were not rendered (e.g. beyond `limit`).
    pub unshown: usize,
}

impl Out {
    pub fn new() -> Out {
        Out::default()
    }

    pub fn h(&mut self, s: impl Into<String>) -> &mut Self {
        self.header.push(s.into());
        self
    }

    pub fn item(&mut self, s: impl Into<String>) -> &mut Self {
        self.items.push(s.into());
        self
    }

    pub fn text(s: impl Into<String>) -> Out {
        let mut o = Out::new();
        o.h(s);
        o
    }

    pub fn render(self, budget: Option<usize>) -> String {
        let mut w = BudgetWriter::new(budget);
        for h in self.header {
            w.header(h);
        }
        for i in self.items {
            w.push(i);
        }
        w.add_unshown(self.unshown);
        w.finish()
    }
}

#[derive(Debug)]
pub enum ToolError {
    BadArgs(String),
    NotFound(String),
    Internal(anyhow::Error),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::BadArgs(m) => write!(f, "invalid arguments: {m}"),
            ToolError::NotFound(m) => write!(f, "{m}"),
            ToolError::Internal(e) => write!(f, "internal error: {e:#}"),
        }
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self {
        ToolError::Internal(e)
    }
}

impl From<rusqlite::Error> for ToolError {
    fn from(e: rusqlite::Error) -> Self {
        ToolError::Internal(e.into())
    }
}

pub type ToolResult = Result<Out, ToolError>;

pub struct Ctx<'a> {
    pub engine: &'a Engine,
    pub conn: &'a Connection,
}

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    pub run: fn(&Ctx, Value) -> ToolResult,
}

pub fn schema_of<T: JsonSchema>() -> Value {
    let mut v = serde_json::to_value(schemars::schema_for!(T)).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.remove("$schema");
        o.remove("title");
        if !o.contains_key("properties") {
            o.insert("properties".into(), Value::Object(Default::default()));
        }
    }
    v
}

pub fn parse_args<T: DeserializeOwned>(v: Value) -> Result<T, ToolError> {
    let v = if v.is_null() { Value::Object(Default::default()) } else { v };
    serde_json::from_value(v).map_err(|e| ToolError::BadArgs(e.to_string()))
}

macro_rules! tool {
    ($name:literal, $desc:expr, $params:ty, $f:path) => {
        ToolDef {
            name: $name,
            description: $desc,
            schema: || schema_of::<$params>(),
            run: |ctx, v| {
                let p: $params = parse_args(v)?;
                $f(ctx, p)
            },
        }
    };
}

pub fn registry() -> Vec<ToolDef> {
    vec![
        // 5.1 Token economy & aggregation
        tool!("batch_query", "Run several CodeGraph tools in one call: operations=[{tool, args}]. Returns each result under a header.", tokens::BatchParams, tokens::batch_query),
        tool!("get_file_skeleton", "Zero-body stub of a file: public declarations and signatures only, with exact line ranges (~60 tokens for large files).", tokens::SkeletonParams, tokens::get_file_skeleton),
        tool!("get_structural_diff", "AST-level diff vs a git ref (default HEAD): symbols added/removed, signature or body changed. Formatting/comment-only edits are ignored.", git::DiffParams, git::get_structural_diff),
        tool!("simulate_signature_change", "Simulate changing a function's parameter count: lists call sites that would break (arg count mismatch), overrides/interface declarations to update, and ambiguous call sites.", tokens::SimulateParams, tokens::simulate_signature_change),
        tool!("get_flow_diagram", "Tiny Mermaid flowchart (3-6 edges) of calls between the given symbols, or around a single symbol. Line style encodes confidence (solid exact, dotted inferred, x unresolved).", tokens::FlowParams, tokens::get_flow_diagram),
        // 5.2 Discovery
        tool!("search_universal", "Ranked FTS5 trigram search over symbols (classes, functions, methods, variables, constants, models). Optional kind filter and token budget.", discovery::SearchParams, discovery::search_universal),
        tool!("find_literal", "Find string/number literals (routes, error messages, config keys, SQL) with exact locations and enclosing symbols.", discovery::LiteralParams, discovery::find_literal),
        tool!("find_variable_or_const", "Find a variable/constant/field declaration plus every read (READS_VAR) and write (WRITES_VAR) site.", discovery::VarParams, discovery::find_variable_or_const),
        // 5.3 Navigation
        tool!("trace_execution_path", "Shortest call path between two symbols (CALLS, HTTP_CALLS, DI), with per-hop line numbers, confidence and evidence.", navigation::TraceParams, navigation::trace_execution_path),
        tool!("trace_fullstack_flow", "Connect a frontend API call or backend route (e.g. 'GET /api/orders' or a symbol) to controllers, services and data side effects.", navigation::FullstackParams, navigation::trace_fullstack_flow),
        tool!("resolve_dependency_injection", "Which concrete class the DI container (.NET, NestJS, FastAPI) resolves for an interface or token, with lifetime, registration site and consumers.", navigation::DiParams, navigation::resolve_dependency_injection),
        tool!("get_call_hierarchy", "Callers or callees of a symbol as a tree (direction: callers|callees|both, depth default 2).", navigation::HierarchyParams, navigation::get_call_hierarchy),
        tool!("get_type_hierarchy", "Base classes / implemented interfaces (supertypes) and subclasses / implementations (subtypes).", navigation::TypeHierarchyParams, navigation::get_type_hierarchy),
        tool!("get_impact_radius", "Transitive blast radius of changing a symbol: dependents by depth, affected files, entrypoints and tests (recursive CTE over reverse edges).", navigation::ImpactParams, navigation::get_impact_radius),
        // 5.4 Behavior
        tool!("get_state_mutations", "Fields, globals, database entities and cache keys mutated by a function (optionally including its callees).", behavior::MutationParams, behavior::get_state_mutations),
        tool!("trace_error_flow", "Where an exception (type name or message fragment) is thrown and where it is caught (try/catch, propagation through callers, global handlers).", behavior::ErrorFlowParams, behavior::trace_error_flow),
        tool!("find_by_capability", "Functions by side-effect tags: DB_READ, DB_WRITE, NETWORK_IO, FILE_IO, ASYNC, AUTH_REQUIRED, CACHE, CONFIG_READ, THROWS, GLOBAL_STATE, LOGGING, EVENT_EMIT.", behavior::CapabilityParams, behavior::find_by_capability),
        tool!("get_entrypoints", "All entrypoints: HTTP routes, main()/top-level programs, CLI commands, event handlers, serverless functions.", behavior::EntrypointParams, behavior::get_entrypoints),
        tool!("find_orphaned_symbols", "Dead-code candidates: symbols with no incoming references (excluding entrypoints, tests, overrides, framework-invoked members).", behavior::OrphanParams, behavior::find_orphaned_symbols),
        // 5.5 Data contracts & extraction
        tool!("get_data_model", "Fields, types, primary/foreign keys and relationships (1:N, N:1, N:N) of a data model (EF Core, Prisma, TypeORM, SQLAlchemy, Django...). Omit model_name to list all.", data::ModelParams, data::get_data_model),
        tool!("get_config_matrix", "Every environment variable / configuration key read by the code, where it is defined, defaults, and missing definitions.", data::ConfigParams, data::get_config_matrix),
        tool!("get_type_contract", "Public contract of a type: base types and member signatures without implementations.", data::ContractParams, data::get_type_contract),
        tool!("get_symbol_body", "Exact source of one symbol with preserved line numbers (optionally without comments, capped by max_lines).", data::BodyParams, data::get_symbol_body),
        tool!("get_file_outline", "Dense hierarchical outline of a file: kinds, names, line ranges, complexity and capability tags.", data::OutlineParams, data::get_file_outline),
        // 5.6 Governance, tests & git
        tool!("get_complexity_hotspots", "Functions ranked by cyclomatic complexity and risk (complexity × fan-in).", governance::HotspotParams, governance::get_complexity_hotspots),
        tool!("validate_dependency_rule", "Check whether source_module may depend on target_module under the architecture rules (.codegraph/architecture.toml or default layering) and list existing violating dependencies.", governance::RuleParams, governance::validate_dependency_rule),
        tool!("get_tests_for_symbol", "Tests that exercise a symbol directly or transitively (recursive CTE up the call graph), plus naming-convention links.", governance::TestsParams, governance::get_tests_for_symbol),
        tool!("get_changed_subgraph", "Symbols touched by `git diff <ref>` (default HEAD, includes uncommitted changes) and their dependents/tests.", git::ChangedParams, git::get_changed_subgraph),
    ]
}

/// Files referenced by a tool call (used for the targeted freshness check).
fn hint_files(conn: &Connection, args: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(obj) = args.as_object() else { return out };
    for (k, v) in obj {
        let mut vals: Vec<&str> = Vec::new();
        match v {
            Value::String(s) => vals.push(s),
            Value::Array(a) => vals.extend(a.iter().filter_map(|x| x.as_str())),
            _ => {}
        }
        for s in vals {
            if k == "file_path" || k == "path" {
                out.extend(query::resolve_file(conn, s).unwrap_or_default().into_iter().take(1));
            } else if matches!(k.as_str(), "symbol_id" | "symbol" | "symbols" | "source" | "target" | "type_id") {
                if let Ok(nodes) = query::find_nodes(conn, s, 2) {
                    out.extend(nodes.into_iter().map(|n| n.file_path));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Runs a tool without freshness checks (used by `batch_query`).
pub fn run_tool(ctx: &Ctx, name: &str, args: Value) -> Result<String, ToolError> {
    let budget = args.get("max_token_budget").and_then(|b| b.as_u64()).map(|b| b as usize);
    let tools = registry();
    let Some(t) = tools.iter().find(|t| t.name == name) else {
        return Err(ToolError::NotFound(format!("unknown tool '{name}'")));
    };
    let out = (t.run)(ctx, args)?;
    Ok(out.render(budget))
}

/// Entry point used by the MCP server: freshness check, then run. Returns (text, is_error).
pub fn call_tool(engine: &Engine, name: &str, args: Value) -> (String, bool) {
    if let Ok(c) = engine.reader() {
        let hints = hint_files(&c, &args);
        drop(c);
        if let Err(e) = engine.ensure_fresh(&hints) {
            tracing::warn!("freshness check failed: {e:#}");
        }
    }
    let conn = match engine.reader() {
        Ok(c) => c,
        Err(e) => return (format!("internal error: {e:#}"), true),
    };
    let ctx = Ctx { engine, conn: &conn };
    match run_tool(&ctx, name, args) {
        Ok(s) => (s, false),
        Err(e) => (e.to_string(), true),
    }
}

// ---------------------------------------------------------------------------
// Shared argument helpers
// ---------------------------------------------------------------------------

/// Resolves a symbol reference (id, qualified name, `Type.member`, name, `file:line`).
pub fn resolve_symbol(conn: &Connection, input: &str) -> Result<Node, ToolError> {
    let nodes = query::find_nodes(conn, input, 12)?;
    match nodes.len() {
        0 => Err(ToolError::NotFound(format!("No symbol matches '{input}'. Use search_universal to find candidates."))),
        1 => Ok(nodes.into_iter().next().unwrap()),
        _ => {
            // Prefer a unique non-module, non-test definition.
            let defs: Vec<&Node> = nodes.iter().filter(|n| n.sym.kind != codegraph_core::SymbolKind::Module).collect();
            if defs.len() == 1 {
                return Ok(defs[0].clone());
            }
            // A type and its own members (e.g. `OrderService` and its constructor): the type wins.
            let types: Vec<&Node> = defs.iter().copied().filter(|n| n.sym.kind.is_type()).collect();
            if types.len() == 1
                && defs.iter().all(|n| n.sym.id == types[0].sym.id || n.sym.parent_id.as_deref() == Some(types[0].sym.id.as_str()))
            {
                return Ok(types[0].clone());
            }
            let mut msg = format!("'{input}' is ambiguous ({} matches); pass one of these ids:\n", nodes.len());
            for n in nodes.iter().take(10) {
                msg.push_str(&format!("{} {}\n", n.sym.id, fmt::node_line(n)));
            }
            Err(ToolError::NotFound(msg.trim_end().to_string()))
        }
    }
}

pub fn resolve_file_arg(conn: &Connection, input: &str) -> Result<String, ToolError> {
    let files = query::resolve_file(conn, input)?;
    match files.len() {
        0 => Err(ToolError::NotFound(format!(
            "File '{input}' is not indexed (supported: .cs .ts .tsx .js .jsx .py .prisma, appsettings*.json, .env*)."
        ))),
        1 => Ok(files.into_iter().next().unwrap()),
        _ => Err(ToolError::NotFound(format!("'{input}' matches several files:\n{}", files.join("\n")))),
    }
}

pub fn nodes_map(conn: &Connection, ids: impl IntoIterator<Item = String>) -> Result<HashMap<String, Node>, ToolError> {
    let ids: Vec<String> = ids.into_iter().collect();
    Ok(query::nodes_by_ids(conn, &ids)?)
}

pub fn wants_json(format: &Option<String>) -> bool {
    format.as_deref().map(|f| f.eq_ignore_ascii_case("json")).unwrap_or(false)
}
