//! Git-backed tools: get_structural_diff, get_changed_subgraph (uses the `git` CLI).

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::Command;

use codegraph_core::diff::{structural_diff, ChangeKind};
use codegraph_core::render::kind_abbrev;
use codegraph_core::{extract_file, Language};
use codegraph_graph::query;
use codegraph_graph::Node;
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;

use super::fmt::{display, node_line};
use super::{nodes_map, Ctx, Out, ToolError, ToolResult};

fn git(root: &Path, args: &[&str]) -> Result<String, ToolError> {
    let out = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| ToolError::Internal(anyhow::anyhow!("git is not available: {e}")))?;
    if !out.status.success() {
        return Err(ToolError::BadArgs(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Changed files (relative to the project root) vs `git_ref`, including untracked ones.
fn changed_files(root: &Path, git_ref: &str) -> Result<(Vec<String>, HashSet<String>), ToolError> {
    let tracked = git(root, &["diff", "--name-only", "--relative", git_ref, "--"])?;
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard"])?;
    let mut files: Vec<String> = tracked.lines().map(|l| l.trim().replace('\\', "/")).filter(|l| !l.is_empty()).collect();
    let new: HashSet<String> = untracked.lines().map(|l| l.trim().replace('\\', "/")).filter(|l| !l.is_empty()).collect();
    files.extend(new.iter().cloned());
    files.retain(|f| Language::from_path(Path::new(f)).is_some() && !codegraph_core::walk::is_ignored_rel_path(f));
    files.sort();
    files.dedup();
    Ok((files, new))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffParams {
    /// Git ref to compare against (default "HEAD"; working tree changes are included).
    pub git_ref: Option<String>,
    /// Optional path filter (substring).
    pub path: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_structural_diff(ctx: &Ctx, p: DiffParams) -> ToolResult {
    let root = &ctx.engine.root;
    let git_ref = p.git_ref.clone().unwrap_or_else(|| "HEAD".into());
    let (files, untracked) = changed_files(root, &git_ref)?;
    let mut out = Out::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut formatting_only = 0;
    for f in &files {
        if let Some(filter) = &p.path {
            if !f.contains(filter.as_str()) {
                continue;
            }
        }
        let lang = Language::from_path(Path::new(f)).unwrap();
        let new_src = std::fs::read_to_string(root.join(f)).ok();
        let old_src = if untracked.contains(f) { None } else { git(root, &["show", &format!("{git_ref}:./{f}")]).ok() };
        let old = extract_file(f, lang, old_src.as_deref().unwrap_or(""));
        let new = extract_file(f, lang, new_src.as_deref().unwrap_or(""));
        match (&old_src, &new_src) {
            (None, Some(_)) => {
                *counts.entry("files added").or_default() += 1;
                let n = new.symbols.iter().filter(|s| s.parent_id.as_deref() == Some(new.module_id())).count();
                out.item(format!("+ file {f} ({n} top-level symbol(s))"));
                continue;
            }
            (Some(_), None) => {
                *counts.entry("files deleted").or_default() += 1;
                out.item(format!("- file {f}"));
                continue;
            }
            _ => {}
        }
        let changes = structural_diff(&old, &new);
        if changes.is_empty() {
            formatting_only += 1;
            out.item(format!("= {f} (formatting/comments only)"));
            continue;
        }
        let mut item = format!("{f}:");
        for c in changes {
            let k = kind_abbrev(c.symbol_kind);
            let line = match &c.kind {
                ChangeKind::Added => {
                    *counts.entry("added").or_default() += 1;
                    format!("\n  + {k} {} :{}-{}", c.qualified_name, c.start_line, c.end_line)
                }
                ChangeKind::Removed => {
                    *counts.entry("removed").or_default() += 1;
                    format!("\n  - {k} {} (was :{}-{})", c.qualified_name, c.start_line, c.end_line)
                }
                ChangeKind::SignatureChanged { old, new } => {
                    *counts.entry("signature").or_default() += 1;
                    format!("\n  ~ sig {k} {} :{}  {old}  →  {new}", c.qualified_name, c.start_line)
                }
                ChangeKind::BodyChanged => {
                    *counts.entry("body").or_default() += 1;
                    format!("\n  ~ body {k} {} :{}-{}", c.qualified_name, c.start_line, c.end_line)
                }
            };
            item.push_str(&line);
        }
        out.item(item);
    }
    out.h(format!(
        "Structural diff vs {git_ref}: {} file(s){}{}",
        files.len(),
        if counts.is_empty() {
            String::new()
        } else {
            format!(" | {}", counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "))
        },
        if formatting_only > 0 { format!(" | {formatting_only} formatting-only") } else { String::new() }
    ));
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangedParams {
    /// Git ref to diff against (default "HEAD"; includes uncommitted changes).
    pub git_ref: Option<String>,
    /// Dependent depth (default 2).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

/// Parses `git diff -U0` output into (file → changed new-side line ranges).
fn changed_ranges(diff: &str) -> BTreeMap<String, Vec<(u32, u32)>> {
    let mut out: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
    let mut cur: Option<String> = None;
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            cur = rest.strip_prefix("b/").map(|s| s.to_string());
        } else if line.starts_with("@@") {
            let Some(f) = &cur else { continue };
            // @@ -a,b +c,d @@
            if let Some(plus) = line.split_whitespace().find(|t| t.starts_with('+')) {
                let spec = plus.trim_start_matches('+');
                let (start, len) = match spec.split_once(',') {
                    Some((s, l)) => (s.parse::<u32>().unwrap_or(0), l.parse::<u32>().unwrap_or(0)),
                    None => (spec.parse::<u32>().unwrap_or(0), 1),
                };
                let end = if len == 0 { start } else { start + len - 1 };
                out.entry(f.clone()).or_default().push((start.max(1), end.max(1)));
            }
        }
    }
    out
}

pub fn get_changed_subgraph(ctx: &Ctx, p: ChangedParams) -> ToolResult {
    let root = &ctx.engine.root;
    let git_ref = p.git_ref.clone().unwrap_or_else(|| "HEAD".into());
    let depth = p.depth.unwrap_or(2).clamp(1, 6);
    let (files, untracked) = changed_files(root, &git_ref)?;
    let diff = git(root, &["diff", "-U0", "--relative", "--no-color", &git_ref, "--"])?;
    let ranges = changed_ranges(&diff);
    let mut changed: Vec<Node> = Vec::new();
    let mut seen = HashSet::new();
    let mut deleted = Vec::new();
    for f in &files {
        let nodes = query::nodes_in_file(ctx.conn, f)?;
        if nodes.is_empty() {
            if !root.join(f).exists() {
                deleted.push(f.clone());
            }
            continue;
        }
        let file_ranges: Vec<(u32, u32)> =
            if untracked.contains(f) { vec![(1, u32::MAX)] } else { ranges.get(f).cloned().unwrap_or_default() };
        // Semantic filter: only symbols whose AST actually changed (formatting/comments ignored).
        let semantic: Option<HashSet<String>> = if untracked.contains(f) {
            None
        } else {
            let lang = Language::from_path(Path::new(f)).unwrap();
            git(root, &["show", &format!("{git_ref}:./{f}")]).ok().map(|old_src| {
                let new_src = std::fs::read_to_string(root.join(f)).unwrap_or_default();
                let old = extract_file(f, lang, &old_src);
                let new = extract_file(f, lang, &new_src);
                structural_diff(&old, &new).into_iter().map(|c| c.qualified_name).collect()
            })
        };
        for (a, b) in file_ranges {
            // Innermost symbols whose range intersects the hunk.
            let mut hits: Vec<&Node> = nodes
                .iter()
                .filter(|n| n.sym.kind != codegraph_core::SymbolKind::Module && n.sym.start_line <= b && n.sym.end_line >= a)
                .collect();
            let inner: Vec<&Node> = hits
                .iter()
                .copied()
                .filter(|n| {
                    !hits
                        .iter()
                        .any(|m| m.sym.parent_id.as_deref() == Some(n.sym.id.as_str()) && m.sym.start_line <= b && m.sym.end_line >= a)
                })
                .collect();
            hits = inner;
            if let Some(sem) = &semantic {
                hits.retain(|n| sem.contains(&n.sym.qualified_name));
            }
            for n in hits {
                if seen.insert(n.sym.id.clone()) {
                    changed.push(n.clone());
                }
            }
        }
    }
    let mut out = Out::new();
    out.h(format!(
        "Changed vs {git_ref}: {} file(s), {} symbol(s){}",
        files.len(),
        changed.len(),
        if deleted.is_empty() { String::new() } else { format!(", {} deleted file(s)", deleted.len()) }
    ));
    for d in &deleted {
        out.item(format!("DELETED {d}"));
    }
    for n in &changed {
        out.item(format!("CHANGED {}", node_line(n)));
    }
    // Dependents via recursive CTE over reverse edges.
    let mut dependents: BTreeMap<String, i64> = BTreeMap::new();
    let sql = "WITH RECURSIVE r(id, d) AS (SELECT ?1, 0 UNION SELECT e.source_id, r.d + 1 FROM r JOIN edge e ON e.target_id = r.id \
               WHERE r.d < ?2 AND e.relation IN ('CALLS','HTTP_CALLS','INHERITS','IMPORTS','READS_VAR','WRITES_VAR','RESOLVES_DI','TESTS')) \
               SELECT id, MIN(d) FROM r WHERE d > 0 GROUP BY id";
    let mut st = ctx.conn.prepare(sql)?;
    for n in &changed {
        for r in st.query_map(params![n.sym.id, depth as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.flatten() {
            if !seen.contains(&r.0) {
                let e = dependents.entry(r.0).or_insert(r.1);
                *e = (*e).min(r.1);
            }
        }
    }
    let dn = nodes_map(ctx.conn, dependents.keys().cloned())?;
    let mut deps: Vec<(&Node, i64)> = dependents.iter().filter_map(|(id, d)| dn.get(id).map(|n| (n, *d))).collect();
    deps.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.file_path.cmp(&b.0.file_path)));
    let tests: Vec<&Node> = deps.iter().filter(|(n, _)| n.sym.is_test).map(|(n, _)| *n).collect();
    out.item(format!("{} dependent(s) within depth {depth}, {} test(s) to run", deps.len(), tests.len()));
    for t in &tests {
        out.item(format!("RUN TEST {}", node_line(t)));
    }
    for (n, d) in deps.iter().filter(|(n, _)| !n.sym.is_test) {
        out.item(format!("d{d} {}", node_line(n)));
    }
    let _ = display;
    Ok(out)
}
