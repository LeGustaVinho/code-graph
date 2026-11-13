//! A compact orientation pass over the indexed project.

mod runtime;

use std::collections::{BTreeMap, BTreeSet};

use codegraph_core::extract::truncate;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use super::discovery::{matches_scope, normalize_scope};
use super::{Ctx, Out, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverviewParams {
    /// Optional root-relative file or directory. Counts and samples only cover this indexed scope.
    pub path: Option<String>,
    /// Show configured Electron and Tauri boot evidence instead of the general overview.
    pub runtime: Option<bool>,
    /// Maximum code areas and entrypoint samples per section (default 8, max 30).
    pub limit: Option<usize>,
    /// Optional output token budget.
    pub max_token_budget: Option<usize>,
}

#[derive(Default)]
struct Inventory {
    code_files: usize,
    config_files: usize,
    vendor_files: usize,
    unindexed_files: usize,
    languages: BTreeMap<String, usize>,
    areas: BTreeMap<String, usize>,
}

/// Group files by at most two directory components below the selected scope.
/// An exact file scope has no directory areas.
fn area<'a>(path: &'a str, scope: &str) -> Option<&'a str> {
    let base = if scope.is_empty() { 0 } else { scope.len() + 1 };
    let relative = path.get(base..)?;
    let Some((dir, _)) = relative.rsplit_once('/') else {
        return Some(if base == 0 { "." } else { &path[..base - 1] });
    };
    let end = dir.match_indices('/').nth(1).map_or(dir.len(), |(i, _)| i);
    Some(&path[..base + end])
}

fn inventory(conn: &Connection, scope: &str) -> rusqlite::Result<Inventory> {
    let mut inventory = Inventory::default();
    let mut stmt = conn.prepare("SELECT file_path, COALESCE(NULLIF(language, ''), 'unknown'), status, is_vendor FROM file_meta")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let file: String = row.get(0)?;
        if !matches_scope(&file, scope) {
            continue;
        }
        let status: String = row.get(2)?;
        let vendor: bool = row.get(3)?;
        let language: String = row.get(1)?;
        if status != "indexed" {
            inventory.unindexed_files += 1;
        } else if vendor {
            inventory.vendor_files += 1;
        } else if language == "config" {
            inventory.config_files += 1;
        } else {
            inventory.code_files += 1;
            *inventory.languages.entry(language).or_default() += 1;
            if let Some(area) = area(&file, scope) {
                *inventory.areas.entry(area.to_owned()).or_default() += 1;
            }
        }
    }
    Ok(inventory)
}

struct Entrypoint {
    kind: String,
    name: String,
    file: String,
    line: u32,
}

fn entrypoints(conn: &Connection, scope: &str) -> rusqlite::Result<Vec<Entrypoint>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(n.entry_kind), ''), 'entry'), n.qualified_name, n.file_path, n.start_line
         FROM node n JOIN file_meta f ON f.file_path = n.file_path
         WHERE n.is_entrypoint = 1 AND n.is_test = 0
           AND COALESCE(n.entry_kind, '') NOT LIKE 'test hook%'
           AND f.status = 'indexed' AND f.is_vendor = 0 AND f.language IS NOT 'config'
           AND f.file_path NOT IN (SELECT file_path FROM node WHERE kind = 'module' AND is_test = 1)
         ORDER BY n.derived, n.file_path, n.start_line, n.id",
    )?;
    let mut rows = stmt.query([])?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        let file: String = row.get(2)?;
        if matches_scope(&file, scope) {
            entries.push(Entrypoint { kind: row.get(0)?, name: row.get(1)?, file, line: row.get(3)? });
        }
    }
    Ok(entries)
}

fn ranked_counts(counts: &BTreeMap<String, usize>) -> Vec<(&str, usize)> {
    let mut ranked: Vec<_> = counts.iter().map(|(name, &count)| (name.as_str(), count)).collect();
    ranked.sort_by(|(a, a_count), (b, b_count)| b_count.cmp(a_count).then_with(|| a.cmp(b)));
    ranked
}

fn rank_areas<'a>(counts: &'a BTreeMap<String, usize>, scope: &str) -> Vec<(&'a str, usize)> {
    let mut roots = BTreeMap::<&str, usize>::new();
    let mut ranked: Vec<_> = ranked_counts(counts)
        .into_iter()
        .map(|(path, count)| {
            let relative = if scope.is_empty() { path } else { path.get(scope.len() + 1..).unwrap_or(".") };
            let root = relative.split('/').next().unwrap_or(relative);
            let round = roots.entry(root).or_default();
            let key = *round;
            *round += 1;
            (key, path, count)
        })
        .collect();
    ranked.sort_by_key(|&(round, _, _)| round);
    ranked.into_iter().map(|(_, path, count)| (path, count)).collect()
}

/// Represent distinct entry kinds first, then different areas before repetitions.
fn rank_entrypoints<'a>(entries: &'a [Entrypoint], scope: &str) -> Vec<&'a Entrypoint> {
    let mut kinds = BTreeSet::new();
    let mut kind_areas = BTreeSet::new();
    let mut files = BTreeMap::<&str, usize>::new();
    let mut ranked: Vec<_> = entries
        .iter()
        .map(|entry| {
            let kind = entry.kind.split_ascii_whitespace().next().unwrap_or("entry");
            let seen_kind = !kinds.insert(kind);
            let seen_area = !kind_areas.insert((kind, area(&entry.file, scope)));
            let file_count = files.entry(&entry.file).or_default();
            let rank = (seen_kind, seen_area, *file_count);
            *file_count += 1;
            (rank, entry)
        })
        .collect();
    ranked.sort_by_key(|&(rank, _)| rank);
    ranked.into_iter().map(|(_, entry)| entry).collect()
}

pub fn get_project_overview(ctx: &Ctx, p: OverviewParams) -> ToolResult {
    let limit = p.limit.unwrap_or(8).clamp(1, 30);
    let scope = normalize_scope(p.path.as_deref().unwrap_or(""))?;
    let scope = scope.as_str();
    if p.runtime.unwrap_or_default() {
        return runtime::render(ctx, scope, limit);
    }
    // The watcher may commit between queries. Keep coverage and samples on one snapshot.
    let tx = ctx.conn.unchecked_transaction()?;
    let inventory = inventory(&tx, scope)?;
    let entries = entrypoints(&tx, scope)?;
    tx.commit()?;

    let mut out = Out::new();
    let title = if scope.is_empty() { "Indexed project".to_owned() } else { format!("Indexed scope {scope}") };
    out.h(format!(
        "{title} | code files {} | config {} | vendor {} | unindexed records {}",
        inventory.code_files, inventory.config_files, inventory.vendor_files, inventory.unindexed_files
    ));
    if !scope.is_empty() && inventory.code_files + inventory.config_files + inventory.vendor_files + inventory.unindexed_files == 0 {
        out.item("No file records in the index match this scope.");
    }
    let languages = ranked_counts(&inventory.languages).into_iter().map(|(lang, n)| format!("{lang}={n}")).collect::<Vec<_>>().join(" ");
    out.item(format!("Languages (code files): {}", if languages.is_empty() { "none" } else { &languages }));
    let areas = rank_areas(&inventory.areas, scope);
    let entries = rank_entrypoints(&entries, scope);
    out.item(format!("Code areas: {} | Entrypoints: {} (non-test)", areas.len(), entries.len()));
    for i in 0..limit.min(areas.len().max(entries.len())) {
        if let Some(&(path, count)) = areas.get(i) {
            out.item(format!("  dir {path}/ ({count} files)"));
        }
        if let Some(entry) = entries.get(i) {
            let kind = truncate(&entry.kind, 80);
            let name = truncate(&entry.name, 100);
            out.item(format!("  {kind} {name} {}:{}", entry.file, entry.line));
        }
    }
    out.unshown = areas.len().saturating_sub(limit) + entries.len().saturating_sub(limit);
    Ok(out)
}
