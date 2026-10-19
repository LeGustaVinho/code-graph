//! AST-level structural diff between two versions of a file.
//!
//! Symbols are matched by (kind, qualified name, overload index). Bodies are
//! compared through their comment/whitespace-insensitive token hashes, so pure
//! formatting or comment edits never show up as changes.

use std::collections::HashMap;

use crate::model::{FileFacts, Symbol, SymbolKind};

#[derive(Clone, Debug, PartialEq)]
pub enum ChangeKind {
    Added,
    Removed,
    SignatureChanged { old: String, new: String },
    BodyChanged,
}

#[derive(Clone, Debug)]
pub struct StructuralChange {
    pub kind: ChangeKind,
    pub symbol_kind: SymbolKind,
    pub qualified_name: String,
    pub start_line: u32,
    pub end_line: u32,
}

fn key_map(facts: &FileFacts) -> HashMap<(SymbolKind, String, u32), &Symbol> {
    let mut counts: HashMap<(SymbolKind, String), u32> = HashMap::new();
    let mut map = HashMap::new();
    for s in &facts.symbols {
        if s.kind == SymbolKind::Module {
            continue;
        }
        let c = counts.entry((s.kind, s.qualified_name.clone())).or_insert(0);
        map.insert((s.kind, s.qualified_name.clone(), *c), s);
        *c += 1;
    }
    map
}

pub fn structural_diff(old: &FileFacts, new: &FileFacts) -> Vec<StructuralChange> {
    let om = key_map(old);
    let nm = key_map(new);
    let mut out = Vec::new();
    for (k, n) in &nm {
        match om.get(k) {
            None => out.push(StructuralChange {
                kind: ChangeKind::Added,
                symbol_kind: n.kind,
                qualified_name: n.qualified_name.clone(),
                start_line: n.start_line,
                end_line: n.end_line,
            }),
            Some(o) => {
                if o.signature != n.signature {
                    out.push(StructuralChange {
                        kind: ChangeKind::SignatureChanged { old: o.signature.clone(), new: n.signature.clone() },
                        symbol_kind: n.kind,
                        qualified_name: n.qualified_name.clone(),
                        start_line: n.start_line,
                        end_line: n.end_line,
                    });
                } else if !n.kind.is_type() && o.body_hash != n.body_hash {
                    // Containers are not flagged: their changed members are reported individually.
                    out.push(StructuralChange {
                        kind: ChangeKind::BodyChanged,
                        symbol_kind: n.kind,
                        qualified_name: n.qualified_name.clone(),
                        start_line: n.start_line,
                        end_line: n.end_line,
                    });
                }
            }
        }
    }
    for (k, o) in &om {
        if !nm.contains_key(k) {
            out.push(StructuralChange {
                kind: ChangeKind::Removed,
                symbol_kind: o.kind,
                qualified_name: o.qualified_name.clone(),
                start_line: o.start_line,
                end_line: o.end_line,
            });
        }
    }
    out.sort_by(|a, b| a.start_line.cmp(&b.start_line).then(a.qualified_name.cmp(&b.qualified_name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_file;
    use crate::lang::Language;

    #[test]
    fn formatting_and_comments_are_ignored() {
        let a = "export function f(a: number) {\n  return a + 1;\n}\n";
        let b = "// a comment\nexport function f(a: number) {\n\n    return a   +   1; // trailing\n}\n";
        let fa = extract_file("x.ts", Language::TypeScript, a);
        let fb = extract_file("x.ts", Language::TypeScript, b);
        assert!(structural_diff(&fa, &fb).is_empty(), "{:?}", structural_diff(&fa, &fb));
    }

    #[test]
    fn detects_added_removed_and_changed() {
        let a = "export function f(a: number) { return a; }\nexport function g() {}\n";
        let b = "export function f(a: number, b: number) { return a; }\nexport function h() { return 2; }\n";
        let d = structural_diff(&extract_file("x.ts", Language::TypeScript, a), &extract_file("x.ts", Language::TypeScript, b));
        assert!(d.iter().any(|c| c.qualified_name == "f" && matches!(c.kind, ChangeKind::SignatureChanged { .. })));
        assert!(d.iter().any(|c| c.qualified_name == "g" && c.kind == ChangeKind::Removed));
        assert!(d.iter().any(|c| c.qualified_name == "h" && c.kind == ChangeKind::Added));
    }
}
