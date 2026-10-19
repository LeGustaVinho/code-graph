//! Token-lean renderings of a file's symbols: zero-body skeletons and outlines.

use std::collections::HashMap;

use crate::lang::{parse, Language};
use crate::model::{Symbol, SymbolKind};

fn children_map(symbols: &[Symbol]) -> HashMap<String, Vec<&Symbol>> {
    let mut map: HashMap<String, Vec<&Symbol>> = HashMap::new();
    for s in symbols {
        if let Some(p) = &s.parent_id {
            map.entry(p.clone()).or_default().push(s);
        }
    }
    for v in map.values_mut() {
        v.sort_by_key(|s| (s.start_line, s.end_line));
    }
    map
}

fn is_public(s: &Symbol) -> bool {
    !(s.visibility == "private" || s.visibility == "protected" && false)
}

fn skeleton_line(s: &Symbol, lang: Language) -> String {
    match s.kind {
        SymbolKind::Class | SymbolKind::Interface | SymbolKind::Struct | SymbolKind::Enum | SymbolKind::DataModel => {
            if lang == Language::Python {
                format!("{}:", s.signature)
            } else {
                s.signature.clone()
            }
        }
        SymbolKind::Test => format!("test \"{}\"", s.name),
        _ => {
            let mut sig = s.signature.clone();
            if lang == Language::CSharp && s.is_static && !sig.starts_with("const ") && s.kind != SymbolKind::Const {
                sig = format!("static {sig}");
            }
            if lang == Language::CSharp && s.is_async && !sig.starts_with("async") {
                sig = format!("async {sig}");
            }
            sig
        }
    }
}

/// Zero-body stub: public declarations and signatures only.
pub fn skeleton(path: &str, line_count: u32, lang: Language, symbols: &[Symbol]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(module) = symbols.iter().find(|s| s.kind == SymbolKind::Module) else { return out };
    out.push(format!("// {path} ({line_count} lines, {})", lang.as_str()));
    if let Some(ns) = symbols.iter().find_map(|s| s.namespace.clone()) {
        out.push(format!("namespace {ns}"));
    }
    let map = children_map(symbols);
    fn rec(id: &str, depth: usize, map: &HashMap<String, Vec<&Symbol>>, lang: Language, out: &mut Vec<String>) {
        let Some(kids) = map.get(id) else { return };
        for s in kids {
            if !is_public(s) {
                continue;
            }
            // Locals bound at module level that are not exported add noise.
            if depth == 0 && matches!(s.kind, SymbolKind::Variable) && !s.is_exported {
                continue;
            }
            let indent = "  ".repeat(depth);
            let has_kids = map.get(&s.id).map(|k| k.iter().any(|c| is_public(c))).unwrap_or(false);
            let line_ref =
                if s.end_line > s.start_line { format!(" :{}-{}", s.start_line, s.end_line) } else { format!(" :{}", s.start_line) };
            let is_type = s.kind.is_type();
            if is_type && has_kids && s.kind != SymbolKind::Enum {
                let open = if lang == Language::Python { "" } else { " {" };
                out.push(format!("{indent}{}{open}{line_ref}", skeleton_line(s, lang)));
                rec(&s.id, depth + 1, map, lang, out);
                if lang != Language::Python {
                    out.push(format!("{indent}}}"));
                }
            } else if s.kind == SymbolKind::Enum {
                let members: Vec<String> = map.get(&s.id).map(|k| k.iter().map(|c| c.name.clone()).collect()).unwrap_or_default();
                out.push(format!("{indent}{} {{ {} }}{line_ref}", skeleton_line(s, lang), members.join(", ")));
            } else if s.kind == SymbolKind::Test {
                out.push(format!("{indent}{}{line_ref}", skeleton_line(s, lang)));
                rec(&s.id, depth + 1, map, lang, out);
            } else {
                out.push(format!("{indent}{}{line_ref}", skeleton_line(s, lang)));
            }
        }
    }
    rec(&module.id, 0, &map, lang, &mut out);
    out
}

pub fn kind_abbrev(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Module => "mod",
        SymbolKind::Class => "class",
        SymbolKind::Interface => "iface",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::TypeAlias => "type",
        SymbolKind::Method => "m",
        SymbolKind::Constructor => "ctor",
        SymbolKind::Function => "fn",
        SymbolKind::Property => "prop",
        SymbolKind::Field => "field",
        SymbolKind::Variable => "var",
        SymbolKind::Const => "const",
        SymbolKind::Test => "test",
        SymbolKind::DataModel => "model",
        SymbolKind::Literal => "lit",
    }
}

/// Hierarchical outline including private members, complexity and capability tags.
pub fn outline(path: &str, line_count: u32, lang: Language, symbols: &[Symbol]) -> Vec<String> {
    let mut out = Vec::new();
    let Some(module) = symbols.iter().find(|s| s.kind == SymbolKind::Module) else { return out };
    out.push(format!("{path} [{}, {line_count} lines]", lang.as_str()));
    let map = children_map(symbols);
    fn rec(id: &str, depth: usize, map: &HashMap<String, Vec<&Symbol>>, out: &mut Vec<String>) {
        let Some(kids) = map.get(id) else { return };
        for s in kids {
            let indent = "  ".repeat(depth);
            let range = if s.end_line > s.start_line { format!(":{}-{}", s.start_line, s.end_line) } else { format!(":{}", s.start_line) };
            let mut extra = String::new();
            if s.kind.is_callable() && s.complexity > 1 {
                extra.push_str(&format!(" cc{}", s.complexity));
            }
            if !s.capabilities.is_empty() {
                extra.push_str(&format!(" [{}]", s.capabilities.iter().cloned().collect::<Vec<_>>().join(",")));
            }
            if s.visibility == "private" {
                extra.push_str(" -priv");
            }
            if let Some(e) = &s.entry_kind {
                extra.push_str(&format!(" <{e}>"));
            }
            let label = match s.kind {
                SymbolKind::Field | SymbolKind::Property | SymbolKind::Variable | SymbolKind::Const => match &s.type_ref {
                    Some(t) => format!("{}: {}", s.name, t),
                    None => s.name.clone(),
                },
                SymbolKind::Test => format!("\"{}\"", s.name),
                _ => s.name.clone(),
            };
            out.push(format!("{indent}{} {label} {range}{extra}", kind_abbrev(s.kind)));
            rec(&s.id, depth + 1, map, out);
        }
    }
    rec(&module.id, 0, &map, &mut out);
    out
}

/// Replaces comment bytes with spaces (newlines preserved) so line numbers stay intact.
pub fn strip_comments(lang: Language, source: &str) -> String {
    let Some(tree) = parse(lang, source) else { return source.to_string() };
    let mut ranges = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        if n.kind().contains("comment") {
            ranges.push(n.byte_range());
            continue;
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            stack.push(ch);
        }
    }
    // Python docstrings count as comments too.
    let mut bytes = source.as_bytes().to_vec();
    for r in ranges {
        for b in &mut bytes[r] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| source.to_string())
}
