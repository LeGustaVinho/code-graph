//! Optional evidence for configured desktop boot chains.

mod electron;
mod tauri;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use codegraph_core::extract::{line_of, string_like};
use codegraph_core::walk::{normalize_rel, IgnoreRules, MAX_FILE_BYTES};
use codegraph_core::{lang, Language};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rusqlite::OptionalExtension;
use tree_sitter::{Node, Tree};

use super::{Ctx, Out, ToolResult};
use crate::tools::discovery::matches_scope;

pub(super) fn render(ctx: &Ctx, scope: &str, limit: usize) -> ToolResult {
    let mut out = Out::new();
    let label = if scope.is_empty() { "." } else { scope };
    out.h(format!("Runtime entries under {label} (indexed source, live manifests and HTML)"));
    let Ok(root) = ctx.engine.root.canonicalize() else {
        out.item("Project files are unavailable.");
        return Ok(out);
    };
    let files = Files { ctx, scope, root, ignore: IgnoreRules::load(&ctx.engine.root), live_ignores: RefCell::default() };
    let mut rows = Vec::new();
    electron::collect(&files, &mut rows)?;
    tauri::collect(&files, &mut rows)?;
    if rows.is_empty() {
        out.item("No supported desktop entry evidence in this scope. Dynamic and unindexed entrypoints may exist.");
    }
    out.unshown = rows.len().saturating_sub(limit);
    for row in rows.into_iter().take(limit) {
        out.item(row);
    }
    Ok(out)
}

struct Files<'a, 'c> {
    ctx: &'a Ctx<'c>,
    scope: &'a str,
    root: PathBuf,
    ignore: IgnoreRules,
    live_ignores: RefCell<HashMap<String, Vec<Gitignore>>>,
}

impl Files<'_, '_> {
    fn live(&self, file: &str) -> Option<String> {
        self.read(file, true)
    }

    fn read(&self, file: &str, check_live_ignores: bool) -> Option<String> {
        if !matches_scope(file, self.scope) || self.ignore.is_ignored(file) {
            return None;
        }
        let path = self.root.join(file).canonicalize().ok()?;
        let canonical = normalize_rel(&self.root, &path);
        if !path.starts_with(&self.root)
            || !matches_scope(&canonical, file)
            || !matches_scope(file, &canonical)
            || check_live_ignores && self.live_ignored(file)
        {
            return None;
        }
        let metadata = path.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return None;
        }
        let mut source = String::new();
        File::open(path).ok()?.take(MAX_FILE_BYTES + 1).read_to_string(&mut source).ok()?;
        (source.len() as u64 <= MAX_FILE_BYTES).then_some(source)
    }

    // A live-only anchor doesn't have an index record from the walker. Check
    // nested ignore files on its ancestor chain instead of walking sibling trees.
    // Treat any exclusion as sufficient for these optional excerpts.
    fn live_ignored(&self, file: &str) -> bool {
        let mut cache = self.live_ignores.borrow_mut();
        let target = self.root.join(file);
        let mut subdir = parent(file);
        while !subdir.is_empty() {
            let matchers = cache.entry(subdir.to_owned()).or_insert_with(|| {
                let dir = self.root.join(subdir);
                [".codegraphignore", ".ignore", ".gitignore"]
                    .into_iter()
                    .filter_map(|name| {
                        let path = dir.join(name);
                        let metadata = path.symlink_metadata().ok()?;
                        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
                            return None;
                        }
                        let mut builder = GitignoreBuilder::new(&dir);
                        builder.add(path);
                        builder.build().ok()
                    })
                    .collect()
            });
            if matchers.iter().any(|matcher| matcher.matched_path_or_any_parents(&target, false).is_ignore()) {
                return true;
            }
            subdir = parent(subdir);
        }
        false
    }

    fn indexed(&self, file: &str) -> Option<Source> {
        let hash: String = self
            .ctx
            .conn
            .query_row("SELECT content_hash FROM file_meta WHERE file_path = ?1 AND status = 'indexed' AND is_vendor = 0", [file], |r| {
                r.get(0)
            })
            .optional()
            .ok()??;
        let text = self.read(file, false)?;
        if codegraph_core::hash::content_hash(text.as_bytes()) != hash {
            return None;
        }
        let language = Language::from_path(Path::new(file))?;
        let tree = lang::parse(language, &text)?;
        Some(Source { text, tree })
    }
}

struct Source {
    text: String,
    tree: Tree,
}

impl Source {
    fn root(&self) -> Node<'_> {
        self.tree.root_node()
    }

    fn text<'a>(&'a self, node: Node<'_>) -> &'a str {
        &self.text[node.byte_range()]
    }

    fn literal(&self, node: Node<'_>) -> Option<String> {
        if children(node).any(|n| matches!(n.kind(), "escape_sequence" | "template_substitution")) {
            return None;
        }
        string_like(node, &self.text)
    }
}

fn children(node: Node<'_>) -> std::vec::IntoIter<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).filter(|n| n.kind() != "comment").collect::<Vec<_>>().into_iter()
}

fn nodes(root: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut stack = vec![root];
    std::iter::from_fn(move || {
        let next = stack.pop()?;
        stack.extend(children(next).rev());
        Some(next)
    })
}

fn call_args(node: Node<'_>) -> Vec<Node<'_>> {
    node.child_by_field_name("arguments").map(|n| children(n).collect()).unwrap_or_default()
}

fn callee(node: Node<'_>) -> Option<Node<'_>> {
    (node.kind() == "call_expression" && !node.has_error()).then(|| node.child_by_field_name("function")).flatten()
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') || path.contains(['\\', ':', '?', '#', '\0']) {
        return None;
    }
    let mut parts: Vec<_> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Read literal module script tags, excluding comments, raw-text elements and inert templates.
fn html_modules(source: &str) -> Vec<(u32, &str)> {
    let lower = source.to_ascii_lowercase();
    let mut position = 0;
    let mut template_depth: usize = 0;
    let mut entries = Vec::new();
    while let Some(start) = lower[position..].find('<').map(|offset| position + offset) {
        if lower[start..].starts_with("<!--") {
            let Some(end) = lower[start + 4..].find("-->") else { break };
            position = start + 4 + end + 3;
            continue;
        }
        let mut quote = None;
        let end = source[start + 1..].char_indices().find_map(|(offset, ch)| {
            match ch {
                '\'' | '"' if quote.is_none() => quote = Some(ch),
                _ if quote == Some(ch) => quote = None,
                '>' if quote.is_none() => return Some(start + 1 + offset),
                _ => {}
            }
            None
        });
        let Some(end) = end else { break };
        position = end + 1;
        let tag = &source[start + 1..end];
        let name_end = tag.find(|ch: char| ch.is_ascii_whitespace() || ch == '/').unwrap_or(tag.len());
        let closing = tag.starts_with('/');
        let name = if closing { tag[1..].split_ascii_whitespace().next().unwrap_or("") } else { &tag[..name_end] };
        if name.eq_ignore_ascii_case("template") {
            template_depth = if closing { template_depth.saturating_sub(1) } else { template_depth + 1 };
        }
        if closing {
            continue;
        }
        let name = name.to_ascii_lowercase();
        if name == "base" && template_depth == 0 && html_attribute(&tag[name_end..], "href").is_some() {
            return Vec::new();
        }
        if name == "script" && template_depth == 0 {
            let attrs = &tag[name_end..];
            if html_attribute(attrs, "type").is_some_and(|value| value.eq_ignore_ascii_case("module")) {
                if let Some(path) = html_attribute(attrs, "src").filter(|path| !path.contains('&')) {
                    entries.push((source[..start].bytes().filter(|&b| b == b'\n').count() as u32 + 1, path));
                }
            }
        }
        if matches!(name.as_str(), "script" | "style" | "textarea" | "title" | "xmp" | "iframe" | "noscript") {
            let Some(end) = lower[position..].find(&format!("</{name}")) else { break };
            position += end;
        }
    }
    entries
}

fn html_attribute<'a>(mut attrs: &'a str, key: &str) -> Option<&'a str> {
    loop {
        attrs = attrs.trim_start();
        if attrs.is_empty() {
            return None;
        }
        let end = attrs.find(|ch: char| ch.is_ascii_whitespace() || ch == '=').unwrap_or(attrs.len());
        if end == 0 {
            return None;
        }
        let name = &attrs[..end];
        attrs = attrs[end..].trim_start();
        let value = if let Some(rest) = attrs.strip_prefix('=') {
            attrs = rest.trim_start();
            if let Some(quote) = attrs.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
                let end = attrs[1..].find(quote)? + 1;
                let value = &attrs[1..end];
                attrs = &attrs[end + 1..];
                Some(value)
            } else {
                let end = attrs.find(|ch: char| ch.is_ascii_whitespace()).unwrap_or(attrs.len());
                let value = &attrs[..end];
                attrs = &attrs[end..];
                Some(value)
            }
        } else {
            None
        };
        if name.eq_ignore_ascii_case(key) {
            return value;
        }
    }
}

fn html_entries(files: &Files<'_, '_>, html: &str, web_root: Option<&str>) -> Vec<(String, u32)> {
    let Some(source) = files.live(html) else { return Vec::new() };
    html_modules(&source)
        .into_iter()
        .filter_map(|(line, path)| {
            let source = match path.strip_prefix('/') {
                Some(path) if !path.starts_with('/') => join(web_root?, path),
                Some(_) => None,
                None => join(parent(html), path),
            }?;
            Some((source, line))
        })
        .collect()
}

fn first_call<'a>(source: &'a Source, name: &str) -> Option<Node<'a>> {
    nodes(source.root()).find(|&node| callee(node).is_some_and(|callee| source.text(callee) == name))
}

fn call_line(source: &Source, name: &str) -> Option<u32> {
    first_call(source, name).map(line_of)
}

// Find the top-level JSON property token after serde has validated the manifest.
fn json_property_line(source: &str, key: &str) -> Option<u32> {
    let mut depth = 0;
    let mut string = None;
    let mut escaped = false;
    let mut result = None;
    for (i, ch) in source.char_indices() {
        if let Some(start) = string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                string = None;
                if depth == 1
                    && source[i + 1..].trim_start().starts_with(':')
                    && serde_json::from_str::<String>(&source[start..=i]).ok().as_deref() == Some(key)
                {
                    result = Some(source[..start].bytes().filter(|&b| b == b'\n').count() as u32 + 1);
                }
            }
        } else {
            match ch {
                '"' => string = Some(i),
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                _ => {}
            }
        }
    }
    result
}
