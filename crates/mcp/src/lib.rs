//! CodeGraph MCP server: 28 analytical tools over the semantic code graph, served as
//! JSON-RPC 2.0 over stdio with per-call freshness checks and token budgets.

pub mod engine;
pub mod server;
pub mod tools;

pub use engine::Engine;
pub use tools::{call_tool, registry};
