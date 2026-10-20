//! Project selection: the explorer can start without a project (launcher mode) and
//! open or switch projects at runtime. Also keeps the list of recently opened projects
//! and lists folders so the browser UI can pick one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{FromRequestParts, Query, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use codegraph_graph::Progress;
use codegraph_mcp::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ApiError;

/// Shared server state: the open project (if any) and an in-flight open request.
pub struct AppCtx {
    engine: RwLock<Option<Arc<Engine>>>,
    opening: Mutex<Option<Opening>>,
}

struct Opening {
    path: PathBuf,
    started: Instant,
    error: Option<String>,
    /// Live stage and counts of the indexing run (drives the progress bar).
    progress: Arc<Progress>,
}

pub type AppState = Arc<AppCtx>;

impl AppCtx {
    pub fn new(engine: Option<Arc<Engine>>) -> AppState {
        if let Some(e) = &engine {
            remember(&e.root);
        }
        Arc::new(AppCtx { engine: RwLock::new(engine), opening: Mutex::new(None) })
    }

    pub fn engine(&self) -> Option<Arc<Engine>> {
        self.engine.read().unwrap().clone()
    }
}

/// Extractor for handlers that need an open project; answers 409 `no_project` otherwise.
pub struct Eng(pub Arc<Engine>);

pub struct NoProject;

impl IntoResponse for NoProject {
    fn into_response(self) -> Response {
        (StatusCode::CONFLICT, Json(json!({ "error": "no project is open", "code": "no_project" }))).into_response()
    }
}

impl FromRequestParts<AppState> for Eng {
    type Rejection = NoProject;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        state.engine().map(Eng).ok_or(NoProject)
    }
}

// ------------------------------------------------------------------ recent projects

#[derive(Serialize, Deserialize, Clone)]
struct Recent {
    path: String,
    last_opened: u64,
}

/// Where the recent-projects list lives (`CODEGRAPH_CONFIG_DIR` overrides it).
fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CODEGRAPH_CONFIG_DIR") {
        Some(PathBuf::from(d))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("codegraph"))
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        Some(PathBuf::from(x).join("codegraph"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config").join("codegraph"))
    }
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent.json"))
}

fn load_recent() -> Vec<Recent> {
    recent_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Moves `root` to the top of the recent list (kept to 20 entries).
fn remember(root: &Path) {
    let Some(file) = recent_file() else { return };
    let path = display_path(root);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut list = load_recent();
    list.retain(|r| !r.path.eq_ignore_ascii_case(&path));
    list.insert(0, Recent { path, last_opened: now });
    list.truncate(20);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
        let _ = std::fs::write(file, bytes);
    }
}

fn forget(path: &str) {
    let Some(file) = recent_file() else { return };
    let mut list = load_recent();
    list.retain(|r| !r.path.eq_ignore_ascii_case(path));
    if let Ok(bytes) = serde_json::to_vec_pretty(&list) {
        let _ = std::fs::write(file, bytes);
    }
}

/// Path as shown to people: no `\\?\` verbatim prefix, forward slashes.
fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/")
}

fn is_indexed(p: &Path) -> bool {
    p.join(".codegraph").join("graph.db").is_file()
}

fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| display_path(p))
}

// ------------------------------------------------------------------ handlers

/// Open project, in-flight open, recent projects and handy starting folders.
pub async fn project_info(State(ctx): State<AppState>) -> Json<Value> {
    let open = ctx.engine().map(|e| json!({ "root": display_path(&e.root), "name": dir_name(&e.root) }));
    let opening = ctx.opening.lock().unwrap().as_ref().map(|o| {
        json!({
            "path": display_path(&o.path),
            "elapsed_ms": o.started.elapsed().as_millis() as u64,
            "error": o.error,
            "progress": o.progress.snapshot(),
        })
    });
    let recent: Vec<Value> = load_recent()
        .into_iter()
        .map(|r| {
            let p = PathBuf::from(&r.path);
            json!({ "path": r.path, "name": dir_name(&p), "last_opened": r.last_opened, "exists": p.is_dir(), "indexed": is_indexed(&p) })
        })
        .collect();
    let mut places = Vec::new();
    let mut place = |key: &str, p: Option<PathBuf>| {
        if let Some(p) = p.filter(|p| p.is_dir()) {
            places.push(json!({ "key": key, "path": display_path(&p) }));
        }
    };
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    place("cwd", std::env::current_dir().ok());
    place("home", home.clone());
    place("desktop", home.as_ref().map(|h| h.join("Desktop")));
    place("documents", home.as_ref().map(|h| h.join("Documents")));
    let cwd = std::env::current_dir().ok();
    Json(json!({
        "open": open,
        "opening": opening,
        "recent": recent,
        "places": places,
        "cwd_indexed": cwd.as_deref().map(is_indexed).unwrap_or(false),
    }))
}

#[derive(Deserialize)]
pub struct FsQ {
    path: Option<String>,
}

/// Sub-folders of `path` (drives / filesystem root when empty), for the folder browser.
pub async fn list_dirs(Query(q): Query<FsQ>) -> Result<Json<Value>, ApiError> {
    let path = q.path.unwrap_or_default();
    if path.trim().is_empty() {
        return Ok(Json(json!({ "path": "", "parent": null, "entries": roots() })));
    }
    let dir = PathBuf::from(path.trim());
    if !dir.is_dir() {
        return Err(ApiError(format!("{} is not a folder", dir.display())));
    }
    let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
    let mut entries: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
        .filter(|(name, _)| !name.starts_with('.') && !name.starts_with('$') && name != "node_modules")
        .collect();
    entries.sort_by_key(|(n, _)| n.to_lowercase());
    let truncated = entries.len() > 2000;
    entries.truncate(2000);
    let parent = dir.parent().map(display_path);
    Ok(Json(json!({
        "path": display_path(&dir),
        "parent": parent,
        "indexed": is_indexed(&dir),
        "truncated": truncated,
        "entries": entries.iter().map(|(n, p)| json!({ "name": n, "path": display_path(p), "indexed": is_indexed(p) })).collect::<Vec<_>>(),
    })))
}

fn roots() -> Vec<Value> {
    if cfg!(windows) {
        (b'A'..=b'Z')
            .map(|c| format!("{}:/", c as char))
            .filter(|d| Path::new(d).is_dir())
            .map(|d| json!({ "name": d.trim_end_matches('/'), "path": d, "indexed": false }))
            .collect()
    } else {
        vec![json!({ "name": "/", "path": "/", "indexed": false })]
    }
}

#[derive(Deserialize)]
pub struct OpenReq {
    path: String,
}

/// Starts indexing `path` in the background; the project switches over when done.
pub async fn open_project(State(ctx): State<AppState>, Json(req): Json<OpenReq>) -> Result<Json<Value>, ApiError> {
    let shown = begin_open(&ctx, PathBuf::from(req.path.trim()))?;
    Ok(Json(json!({ "opening": shown })))
}

/// Indexes `dir` on a blocking thread while the page shows its progress, then makes it the
/// open project. Must run inside the Tokio runtime.
pub fn begin_open(ctx: &AppState, dir: PathBuf) -> Result<String, ApiError> {
    if !dir.is_dir() {
        return Err(ApiError(format!("{} is not a folder", dir.display())));
    }
    let progress = Arc::new(Progress::new());
    {
        let mut op = ctx.opening.lock().unwrap();
        if op.as_ref().is_some_and(|o| o.error.is_none()) {
            return Err(ApiError("another project is still being opened".into()));
        }
        *op = Some(Opening { path: dir.clone(), started: Instant::now(), error: None, progress: progress.clone() });
    }
    let bg = ctx.clone();
    let shown = display_path(&dir);
    tracing::info!("indexing {shown} …");
    tokio::spawn(async move {
        let path = dir.clone();
        let started = Instant::now();
        let res = tokio::task::spawn_blocking(move || Engine::open_with(&path, progress)).await;
        match res {
            Ok(Ok(engine)) => {
                engine.start_watcher();
                remember(&engine.root);
                tracing::info!("opened project {} in {:.1} s", display_path(&engine.root), started.elapsed().as_secs_f32());
                *bg.engine.write().unwrap() = Some(engine);
                *bg.opening.lock().unwrap() = None;
            }
            Ok(Err(e)) => set_error(&bg, e.to_string()),
            Err(e) => set_error(&bg, e.to_string()),
        }
    });
    Ok(shown)
}

fn set_error(ctx: &AppCtx, msg: String) {
    if let Some(o) = ctx.opening.lock().unwrap().as_mut() {
        o.error = Some(msg);
    }
}

#[derive(Deserialize)]
pub struct ForgetReq {
    path: String,
}

pub async fn forget_project(Json(req): Json<ForgetReq>) -> Json<Value> {
    forget(&req.path);
    Json(json!({ "ok": true }))
}
