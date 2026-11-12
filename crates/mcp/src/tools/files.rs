//! Discover indexed paths without reading the source files.

use schemars::JsonSchema;
use serde::Deserialize;

use super::discovery::{matches_scope, normalize_scope};
use super::{Ctx, Out, ToolError, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindFilesParams {
    /// Case-sensitive literal basename or path suffix (e.g. "main" or "renderer/main.ts").
    pub query: String,
    /// Optional root-relative file or directory scope.
    pub path: Option<String>,
    /// Maximum paths to return (default 12, max 100).
    pub limit: Option<usize>,
    pub max_token_budget: Option<usize>,
}

fn path_rank(file: &str, suffix_dir: &str, query: &str) -> Option<u8> {
    let (dir, name) = file.rsplit_once('/').unwrap_or(("", file));
    if !suffix_dir.is_empty() && dir != suffix_dir && !dir.strip_suffix(suffix_dir).is_some_and(|prefix| prefix.ends_with('/')) {
        return None;
    }
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else {
        None
    }
}

fn root_relative(path: &str) -> Result<String, ToolError> {
    if path.trim_start().starts_with(['/', '\\']) {
        return Err(ToolError::BadArgs("path must be relative to project root".into()));
    }
    normalize_scope(path)
}

pub fn find_files(ctx: &Ctx, p: FindFilesParams) -> ToolResult {
    let raw_query = p.query.trim();
    let query = root_relative(raw_query).map_err(|_| ToolError::BadArgs("query must be a relative filename or path suffix".into()))?;
    if query.is_empty() || raw_query.ends_with(['/', '\\']) {
        return Err(ToolError::BadArgs("query must include a filename or part of one".into()));
    }
    let (dir, name) = query.rsplit_once('/').unwrap_or(("", &query));
    let scope = root_relative(p.path.as_deref().unwrap_or(""))?;
    let limit = p.limit.unwrap_or(12).clamp(1, 100);
    let mut matches = Vec::new();
    let mut indexed = 0;
    let mut excluded = 0;
    let mut stmt = ctx.conn.prepare("SELECT file_path, status FROM file_meta")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let file: String = row.get(0)?;
        if !matches_scope(&file, &scope) {
            continue;
        }
        let status: String = row.get(1)?;
        if status != "indexed" {
            excluded += 1;
            continue;
        }
        indexed += 1;
        if let Some(rank) = path_rank(&file, dir, name) {
            matches.push((rank, file.len(), file));
        }
    }
    matches.sort_unstable();
    let total = matches.len();
    let scope = if scope.is_empty() { "." } else { &scope };
    let mut out = Out::new();
    out.h(format!("{total} matching indexed path(s) under {scope} ({indexed} indexed files, {excluded} stale/error excluded)"));
    if total == 0 {
        out.h("No match in the index. Ignored, unsupported and other unindexed files were not searched.");
    }
    for (_, _, file) in matches.into_iter().take(limit) {
        out.item(file);
    }
    out.unshown = total.saturating_sub(limit);
    Ok(out)
}
