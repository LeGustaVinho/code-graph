//! Ultra-compact rendering helpers (exact `path:start-end` coordinates everywhere).

use std::collections::HashMap;

use codegraph_core::render::kind_abbrev;
use codegraph_core::SymbolKind;
use codegraph_graph::{EdgeRow, Node};

/// Short human name: `Type.member` for members, file path for modules.
pub fn display(n: &Node) -> String {
    let s = &n.sym;
    match s.kind {
        SymbolKind::Module => n.file_path.clone(),
        SymbolKind::Test => format!("\"{}\"", s.name),
        SymbolKind::Literal => format!("\"{}\"", s.name),
        SymbolKind::Method | SymbolKind::Constructor | SymbolKind::Field | SymbolKind::Property | SymbolKind::Const => {
            let parts: Vec<&str> = s.qualified_name.rsplitn(2, '.').collect();
            if parts.len() == 2 {
                let owner = parts[1].rsplit('.').next().unwrap_or(parts[1]);
                format!("{owner}.{}", parts[0])
            } else {
                s.name.clone()
            }
        }
        _ => s.name.clone(),
    }
}

pub fn coords(n: &Node) -> String {
    n.coords()
}

/// `m OrderService.PlaceOrderAsync src/Services/OrderService.cs:26-50`
pub fn node_line(n: &Node) -> String {
    format!("{} {} {}", kind_abbrev(n.sym.kind), display(n), coords(n))
}

pub fn node_line_sig(n: &Node) -> String {
    let mut s = node_line(n);
    if !n.sym.signature.is_empty() && n.sym.kind != SymbolKind::Module && n.sym.kind != SymbolKind::Literal {
        s.push_str(&format!(" | {}", n.sym.signature));
    }
    s
}

pub fn short_evidence(e: &EdgeRow, max: usize) -> String {
    codegraph_core::extract::truncate(&e.evidence, max)
}

/// `src/auth.ts:10 AuthStore.login -> src/utils/hash.ts:3 hash [exact: direct import from './hash']`
pub fn edge_line(e: &EdgeRow, nodes: &HashMap<String, Node>) -> String {
    let src = nodes.get(&e.source_id).map(display).unwrap_or_else(|| e.source_id.clone());
    let site = format!("{}:{}", e.source_file, e.line);
    match e.target_id.as_ref().and_then(|t| nodes.get(t)) {
        Some(t) => format!(
            "{site} {src} -> {}:{} {} [{}: {}]",
            t.file_path,
            t.sym.start_line,
            display(t),
            e.confidence.as_str(),
            short_evidence(e, 140)
        ),
        None => format!(
            "{site} {src} -> [{}: {}]",
            e.confidence.as_str(),
            if e.evidence.is_empty() { e.unresolved_symbol.clone().unwrap_or_default() } else { short_evidence(e, 140) }
        ),
    }
}

pub fn conf_arrow(c: codegraph_semantic::Confidence) -> &'static str {
    match c {
        codegraph_semantic::Confidence::Exact => "-->",
        codegraph_semantic::Confidence::Inferred => "-.->",
        codegraph_semantic::Confidence::Unresolved => "--x",
    }
}
