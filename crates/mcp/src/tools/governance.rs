//! 5.6 Governance & tests: complexity hotspots, architecture rules, tests for a symbol.

use std::collections::HashSet;

use codegraph_graph::store::{node_from_row, NODE_COLS};
use codegraph_graph::Node;
use codegraph_semantic::arch::ArchGuard;
use rusqlite::params;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::fmt::{display, node_line};
use super::{nodes_map, resolve_symbol, wants_json, Ctx, Out, ToolResult};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HotspotParams {
    /// Minimum cyclomatic complexity (default 5).
    pub min_complexity: Option<u32>,
    /// Maximum results (default 20).
    pub limit: Option<usize>,
    /// Optional module filter (path fragment).
    pub module: Option<String>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_complexity_hotspots(ctx: &Ctx, p: HotspotParams) -> ToolResult {
    let min = p.min_complexity.unwrap_or(5);
    let limit = p.limit.unwrap_or(20).clamp(1, 200);
    let sql = format!(
        "SELECT {}, (SELECT COUNT(*) FROM edge e WHERE e.target_id = n.id AND e.relation IN ('CALLS', 'HTTP_CALLS')) AS fan_in \
         FROM node n WHERE n.derived = 0 AND n.kind IN ('function', 'method', 'constructor', 'test') AND n.cyclomatic_complexity >= ?1 \
         AND (?2 IS NULL OR n.file_path LIKE '%' || ?2 || '%') ORDER BY n.cyclomatic_complexity DESC LIMIT 500",
        NODE_COLS.split(", ").map(|c| format!("n.{c}")).collect::<Vec<_>>().join(", ")
    );
    let mut st = ctx.conn.prepare(&sql)?;
    let mut rows: Vec<(Node, i64)> =
        st.query_map(params![min, p.module], |r| Ok((node_from_row(r)?, r.get::<_, i64>(28)?)))?.flatten().collect();
    let risk = |n: &Node, fan_in: i64| n.sym.complexity as f64 * (1.0 + (1.0 + fan_in as f64).ln());
    rows.sort_by(|a, b| {
        b.0.sym.complexity.cmp(&a.0.sym.complexity).then(risk(&b.0, b.1).partial_cmp(&risk(&a.0, a.1)).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut out = Out::new();
    out.h(format!("{} function(s) with complexity ≥ {min} (risk = cc × (1 + ln(1 + fan-in)))", rows.len()));
    let json = wants_json(&p.format);
    for (n, fan_in) in rows.iter().take(limit) {
        let loc = n.sym.end_line.saturating_sub(n.sym.start_line) + 1;
        let r = risk(n, *fan_in);
        if json {
            out.item(json!({ "name": display(n), "coords": n.coords(), "complexity": n.sym.complexity, "loc": loc, "fan_in": fan_in, "risk": (r * 10.0).round() / 10.0 }).to_string());
        } else {
            out.item(format!("cc{} loc{loc} in{fan_in} risk{r:.1} {} {}", n.sym.complexity, display(n), n.coords()));
        }
    }
    out.unshown = rows.len().saturating_sub(limit);
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleParams {
    /// Importing module: path / folder (e.g. "src/Domain") or a layer name (e.g. "domain").
    pub source_module: String,
    /// Imported module: path / folder or layer name.
    pub target_module: String,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn validate_dependency_rule(ctx: &Ctx, p: RuleParams) -> ToolResult {
    let guard = ArchGuard::load(&ctx.engine.root);
    let verdict = guard.check(&p.source_module, &p.target_module);
    let mut out = Out::new();
    out.h(format!(
        "{}: {} ({}) → {} ({}) — {}",
        if verdict.allowed { "ALLOWED" } else { "VIOLATION" },
        p.source_module,
        verdict.from_layer.clone().unwrap_or_else(|| "?".into()),
        p.target_module,
        verdict.to_layer.clone().unwrap_or_else(|| "?".into()),
        verdict.rule
    ));
    // Existing dependencies between the two modules.
    let mut st = ctx.conn.prepare(
        "SELECT DISTINCT source_file, target_file, relation, line_number FROM edge WHERE target_file IS NOT NULL AND source_file <> target_file \
         AND relation IN ('IMPORTS', 'CALLS', 'INHERITS', 'READS_VAR', 'WRITES_VAR') ORDER BY source_file, line_number",
    )?;
    let rows: Vec<(String, String, String, u32)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.flatten().collect();
    let matches = |file: &str, module: &str, layer: &Option<String>| -> bool {
        let m = module.replace('\\', "/");
        file.starts_with(m.trim_start_matches("./"))
            || file.contains(&format!("/{}/", m.trim_matches('/')))
            || (layer.is_some() && guard.classify(file) == *layer && guard.classify(module) == *layer && !module.contains('/'))
    };
    let existing: Vec<&(String, String, String, u32)> = rows
        .iter()
        .filter(|(s, t, _, _)| matches(s, &p.source_module, &verdict.from_layer) && matches(t, &p.target_module, &verdict.to_layer))
        .collect();
    out.h(format!("{} existing dependency edge(s) from {} to {}", existing.len(), p.source_module, p.target_module));
    for (s, t, rel, line) in existing.iter().take(25) {
        out.item(format!("{}{s}:{line} -{rel}-> {t}", if verdict.allowed { "" } else { "VIOLATES " }));
    }
    // Project-wide violations of any rule (quick governance summary).
    let mut seen = HashSet::new();
    let mut total_violations = 0usize;
    for (s, t, _, _) in &rows {
        if !seen.insert((s.clone(), t.clone())) {
            continue;
        }
        if !guard.check(s, t).allowed {
            total_violations += 1;
        }
    }
    out.item(format!("project-wide: {total_violations} file pair(s) violate architecture rules ({})", guard.source));
    Ok(out)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestsParams {
    /// Symbol id or name (types include their members).
    pub symbol_id: String,
    /// Maximum hops between a test and the symbol (default 4).
    pub depth: Option<usize>,
    pub max_token_budget: Option<usize>,
    pub format: Option<String>,
}

pub fn get_tests_for_symbol(ctx: &Ctx, p: TestsParams) -> ToolResult {
    let n = resolve_symbol(ctx.conn, &p.symbol_id)?;
    let depth = p.depth.unwrap_or(4).clamp(1, 10);
    let sql = "WITH RECURSIVE seeds(id) AS (SELECT ?1 UNION SELECT id FROM node WHERE parent_id = ?1), \
               up(id, d) AS (SELECT id, 0 FROM seeds UNION SELECT e.source_id, up.d + 1 FROM up JOIN edge e ON e.target_id = up.id \
               WHERE up.d < ?2 AND e.relation IN ('CALLS', 'HTTP_CALLS', 'TESTS', 'RESOLVES_DI')) \
               SELECT up.id, MIN(up.d) FROM up JOIN node n ON n.id = up.id WHERE n.is_test = 1 GROUP BY up.id ORDER BY 2";
    let mut st = ctx.conn.prepare(sql)?;
    let rows: Vec<(String, i64)> = st.query_map(params![n.sym.id, depth as i64], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    let nodes = nodes_map(ctx.conn, rows.iter().map(|r| r.0.clone()))?;
    let direct: HashSet<String> =
        codegraph_graph::query::edges_in(ctx.conn, &n.sym.id, &["TESTS"])?.into_iter().map(|e| e.source_id).collect();
    let mut out = Out::new();
    out.h(format!("{} test(s) reach {} {} (≤ {depth} hops)", rows.len(), display(&n), n.coords()));
    for (id, d) in rows {
        let Some(t) = nodes.get(&id) else { continue };
        let how = if direct.contains(&id) { "direct" } else { "transitive" };
        out.item(format!("{d} hop(s) {how} {}", node_line(t)));
    }
    Ok(out)
}
