//! A compact orientation pass over the indexed project.

use std::collections::{BTreeMap, BTreeSet};

use codegraph_core::extract::truncate;
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Ctx, Out, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverviewParams {
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

/// Group files by at most two directory components, retaining repository-root files.
fn area(path: &str) -> &str {
    let Some((dir, _)) = path.rsplit_once('/') else { return "." };
    dir.match_indices('/').nth(1).map_or(dir, |(end, _)| &dir[..end])
}

fn inventory(conn: &Connection) -> rusqlite::Result<Inventory> {
    let mut inventory = Inventory::default();
    let mut stmt = conn.prepare("SELECT file_path, COALESCE(NULLIF(language, ''), 'unknown'), status, is_vendor FROM file_meta")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
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
            let file: String = row.get(0)?;
            inventory.code_files += 1;
            *inventory.languages.entry(language).or_default() += 1;
            *inventory.areas.entry(area(&file).to_owned()).or_default() += 1;
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

fn entrypoints(conn: &Connection) -> rusqlite::Result<Vec<Entrypoint>> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(NULLIF(TRIM(n.entry_kind), ''), 'entry'), n.qualified_name, n.file_path, n.start_line
         FROM node n JOIN file_meta f ON f.file_path = n.file_path
         WHERE n.is_entrypoint = 1 AND n.is_test = 0
           AND COALESCE(n.entry_kind, '') NOT LIKE 'test hook%'
           AND f.status = 'indexed' AND f.is_vendor = 0 AND f.language IS NOT 'config'
           AND f.file_path NOT IN (SELECT file_path FROM node WHERE kind = 'module' AND is_test = 1)
         ORDER BY n.derived, n.file_path, n.start_line, n.id",
    )?;
    let rows = stmt.query_map([], |row| Ok(Entrypoint { kind: row.get(0)?, name: row.get(1)?, file: row.get(2)?, line: row.get(3)? }))?;
    rows.collect()
}

fn ranked_counts(counts: &BTreeMap<String, usize>) -> Vec<(&str, usize)> {
    let mut ranked: Vec<_> = counts.iter().map(|(name, &count)| (name.as_str(), count)).collect();
    ranked.sort_by(|(a, a_count), (b, b_count)| b_count.cmp(a_count).then_with(|| a.cmp(b)));
    ranked
}

fn rank_areas(counts: &BTreeMap<String, usize>) -> Vec<(&str, usize)> {
    let mut roots = BTreeMap::<&str, usize>::new();
    let mut ranked: Vec<_> = ranked_counts(counts)
        .into_iter()
        .map(|(path, count)| {
            let root = path.split('/').next().unwrap_or(path);
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
fn rank_entrypoints(entries: &[Entrypoint]) -> Vec<&Entrypoint> {
    let mut kinds = BTreeSet::new();
    let mut kind_areas = BTreeSet::new();
    let mut files = BTreeMap::<&str, usize>::new();
    let mut ranked: Vec<_> = entries
        .iter()
        .map(|entry| {
            let kind = entry.kind.split_ascii_whitespace().next().unwrap_or("entry");
            let seen_kind = !kinds.insert(kind);
            let seen_area = !kind_areas.insert((kind, area(&entry.file)));
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
    // The watcher may commit between queries. Keep coverage and samples on one snapshot.
    let tx = ctx.conn.unchecked_transaction()?;
    let inventory = inventory(&tx)?;
    let entries = entrypoints(&tx)?;
    tx.commit()?;

    let mut out = Out::new();
    out.h(format!(
        "Indexed project | code files {} | config {} | vendor {} | unindexed records {}",
        inventory.code_files, inventory.config_files, inventory.vendor_files, inventory.unindexed_files
    ));
    let languages = ranked_counts(&inventory.languages).into_iter().map(|(lang, n)| format!("{lang}={n}")).collect::<Vec<_>>().join(" ");
    out.item(format!("Languages (code files): {}", if languages.is_empty() { "none" } else { &languages }));
    let areas = rank_areas(&inventory.areas);
    let entries = rank_entrypoints(&entries);
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
