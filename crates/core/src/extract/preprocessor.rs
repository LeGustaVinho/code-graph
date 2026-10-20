//! C# conditional compilation (`#if` / `#elif` / `#else` / `#endif`).
//!
//! The graph is the union of every branch: variant 0 keeps the first branch of each
//! group (as if every condition held), variant k keeps branch k (clamped to the last
//! branch of shorter groups). Each variant is parsed separately; facts that only exist in
//! a later variant are merged into the first one and tagged with their condition. Every
//! removed byte becomes a space (newlines kept), so offsets and line numbers never move.
//!
//! Directives are only recognised outside block comments and multi-line strings (verbatim
//! `@"…"` and raw `"""…"""`), so a `#if` inside a string literal is just text.

use std::collections::{HashMap, HashSet};

use crate::model::*;

/// Most variants parsed per file (branch indices 0..MAX_VARIANTS).
pub const MAX_VARIANTS: usize = 4;

#[derive(Debug, Default)]
pub struct Layout {
    /// Per line (0-based): is it a preprocessor directive?
    directive: Vec<bool>,
    /// Per line: enclosing (group, branch) pairs, outermost first.
    ctx: Vec<Vec<(usize, usize)>>,
    /// Per group: the directive text of every branch (`#if X`, `#elif Y`, `#else`).
    groups: Vec<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq)]
enum Lex {
    Code,
    BlockComment,
    Verbatim,
    Raw(usize),
}

/// For every line: does it start in code (not inside a block comment / multi-line string)?
fn line_starts_in_code(src: &str) -> Vec<bool> {
    let b = src.as_bytes();
    let mut out = vec![true];
    let mut state = Lex::Code;
    let mut i = 0;
    // Single-line constructs (regular strings, chars, line comments) end at the newline.
    let mut line_comment = false;
    let mut in_str: Option<u8> = None;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line_comment = false;
            in_str = None;
            out.push(state == Lex::Code);
            i += 1;
            continue;
        }
        match state {
            Lex::BlockComment => {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    state = Lex::Code;
                    i += 2;
                    continue;
                }
            }
            Lex::Verbatim => {
                if c == b'"' {
                    if b.get(i + 1) == Some(&b'"') {
                        i += 2;
                        continue;
                    }
                    state = Lex::Code;
                }
            }
            Lex::Raw(n) => {
                if c == b'"' {
                    let run = b[i..].iter().take_while(|x| **x == b'"').count();
                    if run >= n {
                        state = Lex::Code;
                    }
                    i += run;
                    continue;
                }
            }
            Lex::Code => {
                if line_comment {
                    i += 1;
                    continue;
                }
                if let Some(q) = in_str {
                    if c == b'\\' {
                        i += 2;
                        continue;
                    }
                    if c == q {
                        in_str = None;
                    }
                    i += 1;
                    continue;
                }
                match c {
                    b'/' if b.get(i + 1) == Some(&b'/') => line_comment = true,
                    b'/' if b.get(i + 1) == Some(&b'*') => {
                        state = Lex::BlockComment;
                        i += 2;
                        continue;
                    }
                    b'"' => {
                        let run = b[i..].iter().take_while(|x| **x == b'"').count();
                        if run >= 3 {
                            state = Lex::Raw(run);
                            i += run;
                            continue;
                        }
                        // `@"…"`, `$@"…"`, `@$"…"`: verbatim, may span lines.
                        let prev = if i > 0 { b[i - 1] } else { 0 };
                        let prev2 = if i > 1 { b[i - 2] } else { 0 };
                        if prev == b'@' || (prev == b'$' && prev2 == b'@') {
                            state = Lex::Verbatim;
                        } else if run == 2 {
                            // Empty string literal `""`.
                            i += 2;
                            continue;
                        } else {
                            in_str = Some(b'"');
                        }
                    }
                    b'\'' => in_str = Some(b'\''),
                    _ => {}
                }
            }
        }
        i += 1;
    }
    out
}

fn directive_word(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let rest = t.strip_prefix('#')?;
    let word: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    Some((word, collapse(t)))
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Layout {
    pub fn parse(src: &str) -> Layout {
        let code = line_starts_in_code(src);
        let mut lay = Layout::default();
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for (n, line) in src.split_inclusive('\n').enumerate() {
            let in_code = code.get(n).copied().unwrap_or(true);
            let dir = if in_code { directive_word(line) } else { None };
            match dir {
                Some((word, text)) => {
                    match word.as_str() {
                        "if" => {
                            lay.groups.push(vec![text]);
                            stack.push((lay.groups.len() - 1, 0));
                        }
                        "elif" | "else" => {
                            if let Some(top) = stack.last_mut() {
                                top.1 += 1;
                                lay.groups[top.0].push(text);
                            }
                        }
                        "endif" => {
                            stack.pop();
                        }
                        _ => {}
                    }
                    lay.directive.push(true);
                    lay.ctx.push(Vec::new());
                }
                None => {
                    lay.directive.push(false);
                    lay.ctx.push(stack.clone());
                }
            }
        }
        lay
    }

    /// Number of variants worth parsing (1 when no group has an alternative branch).
    pub fn variants(&self) -> usize {
        self.groups.iter().map(|g| g.len()).max().unwrap_or(1).clamp(1, MAX_VARIANTS)
    }

    pub fn has_directives(&self) -> bool {
        self.directive.iter().any(|d| *d)
    }

    fn pick(&self, group: usize, variant: usize) -> usize {
        variant.min(self.groups[group].len().saturating_sub(1))
    }

    /// Is the 0-based line kept in `variant`?
    fn active(&self, line: usize, variant: usize) -> bool {
        if self.directive.get(line).copied().unwrap_or(false) {
            return false;
        }
        self.ctx.get(line).map(|c| c.iter().all(|(g, b)| *b == self.pick(*g, variant))).unwrap_or(true)
    }

    /// Kept in `variant` but not in variant 0 (1-based line, as in facts).
    pub fn exclusive(&self, line1: u32, variant: usize) -> bool {
        let l = line1.saturating_sub(1) as usize;
        self.active(l, variant) && !self.active(l, 0)
    }

    /// Directive text of the innermost branch enclosing a 1-based line (`#elif UNITY_ANDROID`).
    pub fn condition(&self, line1: u32) -> Option<String> {
        let l = line1.saturating_sub(1) as usize;
        let (g, b) = *self.ctx.get(l)?.last()?;
        let text = self.groups.get(g)?.get(b)?.clone();
        // `#else` alone says little: name the condition it negates.
        if text == "#else" {
            if let Some(first) = self.groups[g].first() {
                return Some(format!("#else (of {first})"));
            }
        }
        Some(text)
    }

    /// The source with every line outside `variant` (and every directive) blanked.
    pub fn render(&self, src: &str, variant: usize) -> String {
        let mut out: Vec<u8> = Vec::with_capacity(src.len());
        for (n, line) in src.split_inclusive('\n').enumerate() {
            if self.active(n, variant) {
                out.extend_from_slice(line.as_bytes());
            } else {
                out.extend(line.bytes().map(|b| if b == b'\n' || b == b'\r' { b } else { b' ' }));
            }
        }
        String::from_utf8(out).unwrap_or_else(|_| src.to_string())
    }
}

/// Merges the facts that only exist in `variant` into `base`.
pub fn merge_variant(base: &mut FileFacts, v: FileFacts, lay: &Layout, variant: usize) {
    let excl = |line: u32| lay.exclusive(line, variant);
    // Symbol identity across variants: (kind, qualified name, signature), then (kind, qname).
    let mut by_sig: HashMap<(SymbolKind, String, String), String> = HashMap::new();
    let mut by_name: HashMap<(SymbolKind, String), Vec<String>> = HashMap::new();
    for s in &base.symbols {
        by_sig.entry((s.kind, s.qualified_name.clone(), s.signature.clone())).or_insert_with(|| s.id.clone());
        by_name.entry((s.kind, s.qualified_name.clone())).or_default().push(s.id.clone());
    }
    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut new_syms = Vec::new();
    for s in v.symbols {
        let same = by_sig.get(&(s.kind, s.qualified_name.clone(), s.signature.clone())).cloned().or_else(|| {
            let ids = by_name.get(&(s.kind, s.qualified_name.clone()))?;
            (ids.len() == 1 && !excl(s.start_line)).then(|| ids[0].clone())
        });
        match same {
            Some(id) => {
                // Same declaration: capabilities found in this variant still belong to it.
                if let Some(b) = base.symbols.iter_mut().find(|b| b.id == id) {
                    b.capabilities.extend(s.capabilities.iter().cloned());
                }
                id_map.insert(s.id, id);
            }
            None if excl(s.start_line) => {
                let id = if base.symbols.iter().any(|b| b.id == s.id) { format!("{}~{variant}", s.id) } else { s.id.clone() };
                id_map.insert(s.id.clone(), id.clone());
                new_syms.push(Symbol { id, ..s });
            }
            None => {}
        }
    }
    let map = |id: &str| id_map.get(id).cloned().unwrap_or_else(|| id.to_string());
    for mut s in new_syms {
        s.parent_id = s.parent_id.as_deref().map(map);
        if let Some(c) = lay.condition(s.start_line) {
            if !s.attrs.contains(&c) {
                s.attrs.push(c);
            }
        }
        base.symbols.push(s);
    }
    let mut seen: HashSet<(String, String, RefKind, u32, Option<String>)> =
        base.refs.iter().map(|r| (r.enclosing.clone(), r.name.clone(), r.kind, r.line, r.receiver.clone())).collect();
    for mut r in v.refs.into_iter().filter(|r| excl(r.line)) {
        r.enclosing = map(&r.enclosing);
        if seen.insert((r.enclosing.clone(), r.name.clone(), r.kind, r.line, r.receiver.clone())) {
            base.refs.push(r);
        }
    }
    for i in v.imports.into_iter().filter(|i| excl(i.line)) {
        if !base.imports.iter().any(|b| b.module == i.module && b.local == i.local && b.kind == i.kind) {
            base.imports.push(i);
        }
    }
    for mut l in v.literals.into_iter().filter(|l| excl(l.line)) {
        l.enclosing = map(&l.enclosing);
        base.literals.push(l);
    }
    for mut c in v.config.into_iter().filter(|c| excl(c.line)) {
        c.enclosing = c.enclosing.as_deref().map(map);
        base.config.push(c);
    }
    for mut m in v.mutations.into_iter().filter(|m| excl(m.line)) {
        m.enclosing = map(&m.enclosing);
        base.mutations.push(m);
    }
    for mut d in v.di.into_iter().filter(|d| excl(d.line)) {
        d.enclosing = map(&d.enclosing);
        base.di.push(d);
    }
    for mut r in v.routes.into_iter().filter(|r| excl(r.line)) {
        r.handler = map(&r.handler);
        base.routes.push(r);
    }
    for mut h in v.http_calls.into_iter().filter(|h| excl(h.line)) {
        h.enclosing = map(&h.enclosing);
        base.http_calls.push(h);
    }
    for mut g in v.global_handlers.into_iter().filter(|g| excl(g.line)) {
        g.handler = map(&g.handler);
        base.global_handlers.push(g);
    }
}

/// Tags symbols declared inside a conditional branch with that branch's directive.
pub fn tag_conditions(facts: &mut FileFacts, lay: &Layout) {
    for s in facts.symbols.iter_mut().skip(1) {
        if let Some(c) = lay.condition(s.start_line) {
            if !s.attrs.contains(&c) {
                s.attrs.push(c);
            }
        }
    }
}
