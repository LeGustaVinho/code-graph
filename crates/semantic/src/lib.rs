//! CodeGraph semantic engine.
//!
//! Turns per-file syntactic facts into graph edges. Every edge carries an explicit
//! [`Confidence`] and a human-readable `evidence` string explaining *why* the
//! relation exists:
//!
//! * `exact` – proven by a direct import, a declared/inferred static type, the
//!   enclosing class or a DI container registration.
//! * `inferred` – deduced from naming conventions, identical HTTP routes or a
//!   unique structural match.
//! * `unresolved` – the syntax shows a relation but the destination is ambiguous
//!   (several implementations) or dynamic; `target_id` is `None` and
//!   `unresolved_symbol` names what was referenced.

pub mod arch;
pub mod global;
pub mod index;
pub mod jsconfig;
pub mod models;
pub mod resolve;
pub mod rustcrate;
pub mod unity;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    Exact,
    Inferred,
    Unresolved,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Exact => "exact",
            Confidence::Inferred => "inferred",
            Confidence::Unresolved => "unresolved",
        }
    }

    pub fn parse(s: &str) -> Confidence {
        match s {
            "exact" => Confidence::Exact,
            "inferred" => Confidence::Inferred,
            _ => Confidence::Unresolved,
        }
    }

    /// The weaker of two confidence levels (used when chaining hops).
    pub fn weakest(self, other: Confidence) -> Confidence {
        self.max(other)
    }
}

pub mod relation {
    pub const CALLS: &str = "CALLS";
    pub const INHERITS: &str = "INHERITS";
    pub const IMPORTS: &str = "IMPORTS";
    pub const USES_LITERAL: &str = "USES_LITERAL";
    pub const READS_VAR: &str = "READS_VAR";
    pub const WRITES_VAR: &str = "WRITES_VAR";
    pub const RESOLVES_DI: &str = "RESOLVES_DI";
    pub const HTTP_CALLS: &str = "HTTP_CALLS";
    pub const THROWS: &str = "THROWS";
    pub const CAUGHT_BY: &str = "CAUGHT_BY";
    pub const TESTS: &str = "TESTS";
    /// A type used without being called or inherited (`typeof`, attributes, annotations).
    pub const USES_TYPE: &str = "USES_TYPE";
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub source_id: String,
    pub target_id: Option<String>,
    pub unresolved_symbol: Option<String>,
    pub relation: String,
    pub confidence: Confidence,
    pub evidence: String,
    pub resolver: String,
    pub line: u32,
    pub metadata: Option<serde_json::Value>,
    /// File of the source node (used for cascade invalidation).
    pub source_file: String,
    /// File of the target node, when resolved.
    pub target_file: Option<String>,
    /// Derived edges are recomputed project-wide on every indexing run.
    pub derived: bool,
}

impl Edge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_id: &str,
        source_file: &str,
        target: Option<(&str, &str)>,
        relation: &str,
        confidence: Confidence,
        evidence: impl Into<String>,
        resolver: &str,
        line: u32,
    ) -> Edge {
        Edge {
            source_id: source_id.to_string(),
            target_id: target.map(|t| t.0.to_string()),
            unresolved_symbol: None,
            relation: relation.to_string(),
            confidence,
            evidence: evidence.into(),
            resolver: resolver.to_string(),
            line,
            metadata: None,
            source_file: source_file.to_string(),
            target_file: target.map(|t| t.1.to_string()),
            derived: false,
        }
    }

    pub fn unresolved(
        source_id: &str,
        source_file: &str,
        relation: &str,
        symbol: &str,
        evidence: impl Into<String>,
        resolver: &str,
        line: u32,
    ) -> Edge {
        Edge {
            source_id: source_id.to_string(),
            target_id: None,
            unresolved_symbol: Some(symbol.to_string()),
            relation: relation.to_string(),
            confidence: Confidence::Unresolved,
            evidence: evidence.into(),
            resolver: resolver.to_string(),
            line,
            metadata: None,
            source_file: source_file.to_string(),
            target_file: None,
            derived: false,
        }
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Edge {
        self.metadata = Some(meta);
        self
    }

    pub fn derived(mut self) -> Edge {
        self.derived = true;
        self
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
