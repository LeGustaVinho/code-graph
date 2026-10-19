//! Local Graph Explorer: an Axum HTTP server serving an embedded single-page app
//! (Cytoscape.js) plus a small JSON API over the code graph.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use codegraph_graph::query;
use codegraph_graph::store::{edge_from_row, EdgeRow, EDGE_COLS};
use codegraph_graph::Node;
use codegraph_mcp::Engine;
use rust_embed::RustEmbed;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

type AppState = Arc<Engine>;

pub async fn serve(engine: Arc<Engine>, addr: SocketAddr) -> Result<()> {
    // Background freshness loop: re-index files reported by the watcher.
    let bg = engine.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(800)).await;
            if bg.pending_dirty() > 0 || !bg.is_watching() {
                let e = bg.clone();
                let _ = tokio::task::spawn_blocking(move || e.ensure_fresh(&[])).await;
            }
        }
    });
    let app = Router::new()
        .route("/api/status", get(status))
        .route("/api/search", get(search))
        .route("/api/graph", get(graph))
        .route("/api/node/{id}", get(node_detail))
        .route("/api/overview", get(overview))
        .route("/api/source/{id}", get(source))
        .route("/api/tool/{name}", post(run_tool))
        .route("/api/reindex", post(reindex))
        .fallback(static_handler)
        .with_state(engine);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Graph Explorer listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

struct ApiError(String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": self.0 }))).into_response()
    }
}

impl<E: std::fmt::Display> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.to_string())
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError(e.to_string()))?
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    // Development mode: serve assets from disk (CODEGRAPH_UI_DEV=<assets dir>) without rebuilding.
    if let Some(dir) = std::env::var_os("CODEGRAPH_UI_DEV") {
        let candidate = std::path::Path::new(&dir).join(path);
        if !path.contains("..") {
            if let Ok(bytes) = std::fs::read(&candidate) {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                return Response::builder()
                    .header(header::CONTENT_TYPE, mime.as_ref())
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(Body::from(bytes))
                    .unwrap();
            }
        }
    }
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .header(header::CONTENT_TYPE, mime.as_ref())
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(content.data.into_owned()))
                .unwrap()
        }
        None => match Assets::get("index.html") {
            Some(c) => Response::builder().header(header::CONTENT_TYPE, "text/html").body(Body::from(c.data.into_owned())).unwrap(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

async fn status(State(engine): State<AppState>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let st = engine.status()?;
        Ok(Json(serde_json::to_value(st)?))
    })
    .await
}

fn node_json(n: &Node, degree: Option<i64>) -> Value {
    json!({
        "id": n.sym.id,
        "label": short_label(n),
        "kind": n.sym.kind.as_str(),
        "name": n.sym.name,
        "qualified_name": n.sym.qualified_name,
        "file": n.file_path,
        "start": n.sym.start_line,
        "end": n.sym.end_line,
        "coords": n.coords(),
        "language": n.language,
        "signature": n.sym.signature,
        "complexity": n.sym.complexity,
        "capabilities": n.sym.capabilities,
        "is_entrypoint": n.sym.is_entrypoint,
        "entry_kind": n.sym.entry_kind,
        "is_test": n.sym.is_test,
        "docstring": n.sym.docstring,
        "visibility": n.sym.visibility,
        "degree": degree,
    })
}

fn short_label(n: &Node) -> String {
    match n.sym.kind {
        codegraph_core::SymbolKind::Module => n.file_path.rsplit('/').next().unwrap_or(&n.file_path).to_string(),
        codegraph_core::SymbolKind::Method
        | codegraph_core::SymbolKind::Constructor
        | codegraph_core::SymbolKind::Field
        | codegraph_core::SymbolKind::Property => {
            let parts: Vec<&str> = n.sym.qualified_name.rsplitn(3, '.').collect();
            if parts.len() >= 2 {
                format!("{}.{}", parts[1], parts[0])
            } else {
                n.sym.name.clone()
            }
        }
        _ => n.sym.name.clone(),
    }
}

fn edge_json(e: &EdgeRow) -> Value {
    json!({
        "id": format!("e{}", e.id),
        "source": e.source_id,
        "target": e.target_id.clone().unwrap_or_else(|| format!("unresolved:{}:{}", e.source_id, e.unresolved_symbol.clone().unwrap_or_default())),
        "relation": e.relation,
        "confidence": e.confidence.as_str(),
        "evidence": e.evidence,
        "resolver": e.resolver,
        "line": e.line,
        "site": format!("{}:{}", e.source_file, e.line),
        "unresolved_symbol": e.unresolved_symbol,
        "metadata": e.metadata,
    })
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    kind: Option<String>,
    limit: Option<usize>,
}

async fn search(State(engine): State<AppState>, Query(q): Query<SearchQ>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let conn = engine.reader()?;
        let (rows, total) = query::search(&conn, &q.q, q.kind.as_deref(), q.limit.unwrap_or(30), false)?;
        Ok(Json(json!({ "total": total, "results": rows.iter().map(|n| node_json(n, None)).collect::<Vec<_>>() })))
    })
    .await
}

#[derive(Deserialize)]
struct GraphQ {
    focus: Option<String>,
    depth: Option<usize>,
    relations: Option<String>,
    limit: Option<usize>,
}

const DEFAULT_RELATIONS: &[&str] = &[
    "CALLS",
    "HTTP_CALLS",
    "RESOLVES_DI",
    "INHERITS",
    "THROWS",
    "CAUGHT_BY",
    "TESTS",
    "IMPORTS",
    "READS_VAR",
    "WRITES_VAR",
    "USES_LITERAL",
];

async fn graph(State(engine): State<AppState>, Query(q): Query<GraphQ>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let conn = engine.reader()?;
        let rels: Vec<String> = q
            .relations
            .as_deref()
            .map(|r| r.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_else(|| DEFAULT_RELATIONS.iter().map(|s| s.to_string()).collect());
        let rel_refs: Vec<&str> = rels.iter().map(|s| s.as_str()).collect();
        let limit = q.limit.unwrap_or(250).clamp(10, 2000);
        let mut node_ids: HashSet<String> = HashSet::new();
        let mut edges: Vec<EdgeRow> = Vec::new();
        match q.focus.as_deref().filter(|f| !f.is_empty()) {
            Some(focus) => {
                let depth = q.depth.unwrap_or(1).clamp(1, 4);
                let start = query::find_nodes(&conn, focus, 1)?.into_iter().next().ok_or_else(|| ApiError(format!("no node '{focus}'")))?;
                let mut frontier = vec![start.sym.id.clone()];
                node_ids.insert(start.sym.id.clone());
                // Types: include their members so the neighbourhood is meaningful.
                if start.sym.kind.is_type() || start.sym.kind == codegraph_core::SymbolKind::Module {
                    for c in query::children(&conn, &start.sym.id)? {
                        node_ids.insert(c.sym.id.clone());
                        frontier.push(c.sym.id);
                    }
                }
                let mut seen_edges = HashSet::new();
                for _ in 0..depth {
                    let mut next = Vec::new();
                    for id in frontier.drain(..) {
                        let mut es = query::edges_out(&conn, &id, &rel_refs)?;
                        es.extend(query::edges_in(&conn, &id, &rel_refs)?);
                        for e in es {
                            if !seen_edges.insert(e.id) || node_ids.len() > limit {
                                continue;
                            }
                            for other in [Some(e.source_id.clone()), e.target_id.clone()].into_iter().flatten() {
                                if node_ids.insert(other.clone()) {
                                    next.push(other);
                                }
                            }
                            edges.push(e);
                        }
                    }
                    frontier = next;
                }
            }
            None => {
                // Overview: most connected symbols (excluding modules & literals) and their links.
                let ph = rel_refs.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                let sql = format!(
                    "SELECT id FROM (SELECT n.id, (SELECT COUNT(*) FROM edge e WHERE (e.source_id = n.id OR e.target_id = n.id) AND e.relation IN ({ph})) AS deg \
                     FROM node n WHERE n.kind NOT IN ('module', 'literal', 'field', 'property', 'variable', 'const')) WHERE deg > 0 ORDER BY deg DESC LIMIT {limit}"
                );
                let mut st = conn.prepare(&sql)?;
                let ids: Vec<String> = st.query_map(rusqlite::params_from_iter(rel_refs.iter()), |r| r.get(0))?.flatten().collect();
                node_ids.extend(ids.iter().cloned());
                let id_ph = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
                if !ids.is_empty() {
                    let sql = format!("SELECT {EDGE_COLS} FROM edge WHERE relation IN ({ph}) AND source_id IN ({id_ph}) AND (target_id IN ({id_ph}) OR target_id IS NULL)");
                    let mut args: Vec<String> = rel_refs.iter().map(|s| s.to_string()).collect();
                    args.extend(ids.iter().cloned());
                    args.extend(ids.iter().cloned());
                    let mut st = conn.prepare(&sql)?;
                    edges = st.query_map(rusqlite::params_from_iter(args.iter()), edge_from_row)?.flatten().collect();
                }
            }
        }
        let ids: Vec<String> = node_ids.into_iter().collect();
        let nodes = query::nodes_by_ids(&conn, &ids)?;
        let mut node_list: Vec<Value> = nodes.values().map(|n| node_json(n, None)).collect();
        // Placeholder nodes for unresolved targets.
        let mut extra = HashSet::new();
        for e in &edges {
            if e.target_id.is_none() {
                let tid = format!("unresolved:{}:{}", e.source_id, e.unresolved_symbol.clone().unwrap_or_default());
                if extra.insert(tid.clone()) {
                    node_list.push(json!({ "id": tid, "label": format!("? {}", e.unresolved_symbol.clone().unwrap_or_default()), "kind": "unresolved", "coords": "", "file": "" }));
                }
            }
        }
        let edges: Vec<Value> = edges.iter().filter(|e| nodes.contains_key(&e.source_id)).map(edge_json).collect();
        Ok(Json(json!({ "nodes": node_list, "edges": edges })))
    })
    .await
}

async fn node_detail(State(engine): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let conn = engine.reader()?;
        let Some(n) = query::node(&conn, &id)? else {
            return Err(ApiError(format!("node {id} not found")));
        };
        let outgoing = query::edges_out(&conn, &id, &[])?;
        let incoming = query::edges_in(&conn, &id, &[])?;
        let others: Vec<String> = outgoing.iter().filter_map(|e| e.target_id.clone()).chain(incoming.iter().map(|e| e.source_id.clone())).collect();
        let nodes = query::nodes_by_ids(&conn, &others)?;
        let summarize = |e: &EdgeRow, other: Option<&String>| {
            let mut v = edge_json(e);
            if let Some(o) = other.and_then(|o| nodes.get(o)) {
                v["other"] = json!({ "id": o.sym.id, "label": short_label(o), "coords": o.coords(), "kind": o.sym.kind.as_str() });
            }
            v
        };
        let out_json: Vec<Value> = outgoing.iter().map(|e| summarize(e, e.target_id.as_ref())).collect();
        let in_json: Vec<Value> = incoming.iter().map(|e| summarize(e, Some(&e.source_id))).collect();
        // Data contract.
        let mut st = conn.prepare("SELECT field_name, field_type, is_primary_key, is_foreign_key, target_model, relation_cardinality, line_number FROM data_model_field WHERE model_node_id = ?1 ORDER BY line_number")?;
        let fields: Vec<Value> = st
            .query_map([&id], |r| {
                Ok(json!({ "name": r.get::<_, String>(0)?, "type": r.get::<_, Option<String>>(1)?, "pk": r.get::<_, bool>(2)?, "fk": r.get::<_, bool>(3)?, "target": r.get::<_, Option<String>>(4)?, "cardinality": r.get::<_, Option<String>>(5)?, "line": r.get::<_, Option<u32>>(6)? }))
            })?
            .flatten()
            .collect();
        let framework: Option<String> = conn.query_row("SELECT framework FROM data_model WHERE model_node_id = ?1", [&id], |r| r.get(0)).ok();
        let mut st = conn.prepare("SELECT http_method, path, framework FROM api_route WHERE handler_id = ?1 ORDER BY file_path, line_number, id")?;
        let routes: Vec<Value> = st.query_map([&id], |r| Ok(json!({ "method": r.get::<_, String>(0)?, "path": r.get::<_, String>(1)?, "framework": r.get::<_, String>(2)? })))?.flatten().collect();
        let mut st = conn.prepare("SELECT target_state, mutation_type, line_number FROM state_mutation WHERE method_node_id = ?1 ORDER BY line_number")?;
        let mutations: Vec<Value> = st.query_map([&id], |r| Ok(json!({ "target": r.get::<_, String>(0)?, "type": r.get::<_, String>(1)?, "line": r.get::<_, u32>(2)? })))?.flatten().collect();
        let children = query::children(&conn, &id)?;
        let parent = match &n.sym.parent_id {
            Some(p) => query::node(&conn, p)?.map(|p| json!({ "id": p.sym.id, "label": short_label(&p), "coords": p.coords() })),
            None => None,
        };
        let mut nj = node_json(&n, None);
        nj["attrs"] = json!(n.sym.attrs);
        nj["bases"] = json!(n.sym.bases);
        nj["type_ref"] = json!(n.sym.type_ref);
        nj["params"] = json!(n.sym.params);
        Ok(Json(json!({
            "node": nj,
            "parent": parent,
            "children": children.iter().map(|c| json!({ "id": c.sym.id, "label": short_label(c), "kind": c.sym.kind.as_str(), "coords": c.coords(), "signature": c.sym.signature })).collect::<Vec<_>>(),
            "outgoing": out_json,
            "incoming": in_json,
            "data_model": if fields.is_empty() { Value::Null } else { json!({ "framework": framework, "fields": fields }) },
            "routes": routes,
            "mutations": mutations,
        })))
    })
    .await
}

async fn run_tool(State(engine): State<AppState>, Path(name): Path<String>, Json(args): Json<Value>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let (text, is_error) = codegraph_mcp::call_tool(&engine, &name, args);
        Ok(Json(json!({ "text": text, "isError": is_error })))
    })
    .await
}

async fn reindex(State(engine): State<AppState>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let stats = engine.reindex_all(false)?;
        Ok(Json(serde_json::to_value(stats)?))
    })
    .await
}

/// Dashboard data: graph statistics plus the lists used by the sidebar views.
async fn overview(State(engine): State<AppState>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let conn = engine.reader()?;
        let status = engine.status()?;
        let pairs = |sql: &str| -> Result<Vec<(String, i64)>, ApiError> {
            let mut st = conn.prepare(sql)?;
            let rows = st.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?.unwrap_or_default(), r.get::<_, i64>(1)?)))?;
            Ok(rows.flatten().collect())
        };
        let kinds = pairs("SELECT kind, COUNT(*) FROM node WHERE kind NOT IN ('module') GROUP BY kind ORDER BY 2 DESC, 1")?;
        let relations = pairs("SELECT relation, COUNT(*) FROM edge GROUP BY relation ORDER BY 2 DESC, 1")?;
        let rel_conf = {
            let mut st = conn.prepare("SELECT relation, confidence, COUNT(*) FROM edge GROUP BY 1, 2")?;
            let rows: Vec<Value> = st
                .query_map([], |r| Ok(json!({ "relation": r.get::<_, String>(0)?, "confidence": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)? })))?
                .flatten()
                .collect();
            rows
        };
        let node_list = |sql: &str| -> Result<Vec<Value>, ApiError> {
            let mut st = conn.prepare(sql)?;
            let rows: Vec<Node> = st.query_map([], codegraph_graph::store::node_from_row)?.flatten().collect();
            Ok(rows.iter().map(|n| node_json(n, None)).collect())
        };
        let cols = codegraph_graph::store::NODE_COLS;
        let entrypoints = node_list(&format!("SELECT {cols} FROM node WHERE is_entrypoint = 1 ORDER BY entry_kind, file_path, start_line, id LIMIT 200"))?;
        let hotspots = node_list(&format!(
            "SELECT {cols} FROM node WHERE kind IN ('function','method','constructor') AND cyclomatic_complexity > 1 ORDER BY cyclomatic_complexity DESC, file_path, start_line, id LIMIT 50"
        ))?;
        let models = {
            let mut st = conn.prepare(&format!(
                "SELECT {}, d.framework, (SELECT COUNT(*) FROM data_model_field f WHERE f.model_node_id = n.id) FROM data_model d JOIN node n ON n.id = d.model_node_id ORDER BY n.name, n.file_path, n.id",
                cols.split(", ").map(|c| format!("n.{c}")).collect::<Vec<_>>().join(", ")
            ))?;
            let rows: Vec<Value> = st
                .query_map([], |r| {
                    let n = codegraph_graph::store::node_from_row(r)?;
                    let mut v = node_json(&n, None);
                    v["framework"] = json!(r.get::<_, Option<String>>(28)?);
                    v["fields"] = json!(r.get::<_, i64>(29)?);
                    Ok(v)
                })?
                .flatten()
                .collect();
            rows
        };
        let unresolved = {
            let mut st = conn.prepare(&format!("SELECT {EDGE_COLS} FROM edge WHERE confidence = 'unresolved' AND relation IN ('CALLS', 'HTTP_CALLS', 'RESOLVES_DI') ORDER BY source_file, line_number, source_id, unresolved_symbol, id LIMIT 200"))?;
            let edges: Vec<EdgeRow> = st.query_map([], edge_from_row)?.flatten().collect();
            let ids: Vec<String> = edges.iter().map(|e| e.source_id.clone()).collect();
            let nodes = query::nodes_by_ids(&conn, &ids)?;
            edges
                .iter()
                .map(|e| {
                    let mut v = edge_json(e);
                    if let Some(n) = nodes.get(&e.source_id) {
                        v["source_label"] = json!(short_label(n));
                        v["source_kind"] = json!(n.sym.kind.as_str());
                    }
                    v
                })
                .collect::<Vec<_>>()
        };
        Ok(Json(json!({
            "status": status,
            "kinds": kinds,
            "relations": relations,
            "relation_confidence": rel_conf,
            "entrypoints": entrypoints,
            "hotspots": hotspots,
            "models": models,
            "unresolved": unresolved,
        })))
    })
    .await
}

/// Source lines of a symbol (read from disk; the freshness loop keeps ranges current).
async fn source(State(engine): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        let conn = engine.reader()?;
        let Some(n) = query::node(&conn, &id)? else {
            return Err(ApiError(format!("node {id} not found")));
        };
        let text = std::fs::read_to_string(engine.root.join(&n.file_path))?;
        let all: Vec<&str> = text.lines().collect();
        let start = n.sym.start_line.max(1) as usize;
        let end = (n.sym.end_line as usize).min(all.len()).min(start + 799);
        let lines: Vec<Value> = (start..=end).map(|i| json!({ "n": i, "text": all.get(i - 1).copied().unwrap_or("") })).collect();
        Ok(Json(json!({
            "file": n.file_path,
            "language": n.language,
            "start": n.sym.start_line,
            "end": n.sym.end_line,
            "truncated": (n.sym.end_line as usize) > end,
            "lines": lines,
        })))
    })
    .await
}
