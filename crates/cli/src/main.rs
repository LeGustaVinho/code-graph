//! `codegraph` — deep semantic code indexer, MCP server and graph explorer.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codegraph_core::lang::{parse, Language};
use codegraph_graph::{index_project_with, IndexOptions, Progress, Stage};
use codegraph_mcp::Engine;

#[derive(Parser)]
#[command(
    name = "codegraph",
    version,
    about = "Deep semantic code graph: indexer, MCP server (stdio) and local graph explorer",
    after_help = "Run without a command to open the Graph Explorer and pick a project folder in the browser."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Index (incrementally) a project directory into <dir>/.codegraph/graph.db
    Index {
        /// Project root (default: current directory)
        path: Option<PathBuf>,
        /// Re-parse every file even if unchanged
        #[arg(long)]
        rebuild: bool,
        /// Print statistics as JSON
        #[arg(long)]
        json: bool,
    },
    /// Run the MCP server over stdio (JSON-RPC 2.0)
    #[command(alias = "serve")]
    Mcp {
        /// Project root (default: $CODEGRAPH_ROOT or current directory)
        #[arg(long)]
        root: Option<PathBuf>,
        /// Positional alternative to --root
        path: Option<PathBuf>,
        /// Disable the file watcher (stat-based freshness checks are used instead)
        #[arg(long)]
        no_watch: bool,
    },
    /// Serve the interactive Graph Explorer (http://localhost:4321)
    Ui {
        /// Project root (default: current directory)
        path: Option<PathBuf>,
        #[arg(long, default_value_t = 4321)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Do not open the browser
        #[arg(long)]
        no_open: bool,
    },
    /// Run one MCP tool from the command line and print its output
    Query {
        /// Tool name (see `codegraph tools`)
        tool: String,
        /// Tool arguments as JSON (e.g. '{"query":"Order"}')
        #[arg(long, default_value = "{}")]
        args: String,
        /// Project root (default: current directory)
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// List the MCP tools (use --json for full input schemas)
    Tools {
        #[arg(long)]
        json: bool,
    },
    /// Index status and graph statistics
    Status { path: Option<PathBuf> },
    /// Print the Tree-sitter syntax tree of a file (debugging)
    DumpAst { file: PathBuf },
    /// Print the syntactic facts extracted from a file as JSON (debugging)
    Facts { file: PathBuf },
}

fn root_or_cwd(p: Option<PathBuf>) -> Result<PathBuf> {
    let p = match p {
        Some(p) => p,
        None => match std::env::var_os("CODEGRAPH_ROOT") {
            Some(r) => PathBuf::from(r),
            None => std::env::current_dir()?,
        },
    };
    if !p.is_dir() {
        anyhow::bail!("{} is not a directory", p.display());
    }
    Ok(codegraph_graph::canonical_root(&p))
}

fn init_logging(default: &str) {
    let filter =
        tracing_subscriber::EnvFilter::try_from_env("CODEGRAPH_LOG").unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        // The classic Windows console (e.g. a double-clicked .exe) prints ANSI codes verbatim.
        .with_ansi(!cfg!(windows) && std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .try_init();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(cmd) = cli.cmd else {
        return launcher();
    };
    match cmd {
        Cmd::Index { path, rebuild, json } => {
            init_logging("warn");
            let root = root_or_cwd(path)?;
            let mut conn = codegraph_graph::open(&root)?;
            let progress = Progress::new();
            let show = !json && std::io::IsTerminal::is_terminal(&std::io::stderr());
            let stats =
                with_terminal_progress(&progress, show, || index_project_with(&root, &mut conn, IndexOptions { rebuild }, &progress))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                let g = codegraph_graph::query::stats(&conn)?;
                println!("Indexed {} in {} ms", root.display(), stats.duration_ms);
                println!(
                    "  files: {} total, {} parsed, {} unchanged, {} removed, {} errors, {} with syntax errors",
                    stats.files_total,
                    stats.files_parsed,
                    stats.files_unchanged,
                    stats.files_removed,
                    stats.parse_errors,
                    stats.syntax_errors
                );
                println!(
                    "  graph: {} nodes, {} edges (exact {}, inferred {}, unresolved {})",
                    g.nodes, g.edges, g.exact, g.inferred, g.unresolved
                );
                println!(
                    "  resolution: {} affected file(s){}",
                    stats.affected_files,
                    if stats.full_resolution { " (full)" } else { " (incremental cascade)" }
                );
                println!("  database: {}", codegraph_graph::db_path(&root).display());
            }
        }
        Cmd::Mcp { root, path, no_watch } => {
            init_logging("warn");
            let root = root_or_cwd(root.or(path))?;
            let engine = Engine::open(&root)?;
            if !no_watch {
                engine.start_watcher();
            }
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(codegraph_mcp::server::serve_stdio(engine))?;
        }
        Cmd::Ui { path, port, host, no_open } => {
            init_logging("info");
            let root = root_or_cwd(path)?;
            let addr: SocketAddr = format!("{host}:{port}").parse().context("invalid host/port")?;
            let url = format!("http://{}:{port}", if host == "0.0.0.0" { "localhost" } else { host.as_str() });
            eprintln!("CodeGraph Explorer: {url} (indexing {} — progress is shown in the browser)", root.display());
            if !no_open {
                open_browser(&url);
            }
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(codegraph_ui::serve_opening(root, addr))?;
        }
        Cmd::Query { tool, args, root } => {
            init_logging("warn");
            let root = root_or_cwd(root)?;
            let engine = Engine::open(&root)?;
            let args: serde_json::Value = serde_json::from_str(&args).context("--args must be valid JSON")?;
            let (text, is_error) = codegraph_mcp::call_tool(&engine, &tool, args);
            println!("{text}");
            if is_error {
                std::process::exit(1);
            }
        }
        Cmd::Tools { json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&codegraph_mcp::server::tools_list())?);
            } else {
                for (i, t) in codegraph_mcp::registry().iter().enumerate() {
                    println!("{:>2}. {:<30} {}", i + 1, t.name, t.description);
                }
            }
        }
        Cmd::Status { path } => {
            init_logging("warn");
            let root = root_or_cwd(path)?;
            let conn = codegraph_graph::open(&root)?;
            let stale = codegraph_graph::stale_files(&root, &conn, None)?;
            let g = codegraph_graph::query::stats(&conn)?;
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "root": root, "stale_files": stale, "stats": g }))?);
        }
        Cmd::DumpAst { file } => {
            let src = std::fs::read_to_string(&file)?;
            let lang = Language::from_path(&file).context("unsupported file type")?;
            let tree = parse(lang, &src).context("no tree-sitter grammar for this file type")?;
            dump(tree.root_node(), &src, 0, None);
        }
        Cmd::Facts { file } => {
            let src = std::fs::read_to_string(&file)?;
            let lang = Language::from_path(&file).context("unsupported file type")?;
            let rel = file.to_string_lossy().replace('\\', "/");
            let facts = codegraph_core::extract_file(&rel, lang, &src);
            println!("{}", serde_json::to_string_pretty(&facts)?);
        }
    }
    Ok(())
}

/// No command (e.g. the executable was double-clicked): serve the explorer without a
/// project on the first free local port and let the browser page pick the folder.
fn launcher() -> Result<()> {
    init_logging("info");
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    rt.block_on(async {
        let mut listener = None;
        for port in 4321..4341 {
            if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
                listener = Some(l);
                break;
            }
        }
        let listener = match listener {
            Some(l) => l,
            None => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.context("no free local port")?,
        };
        let url = format!("http://localhost:{}", listener.local_addr()?.port());
        eprintln!("CodeGraph Explorer: {url}");
        eprintln!("Choose a project folder in the browser. Keep this window open; close it (or press Ctrl+C) to stop.");
        if std::env::var_os("CODEGRAPH_NO_OPEN").is_none() {
            open_browser(&url);
        }
        codegraph_ui::serve_on(None, listener).await
    })
}

/// Runs `work` while redrawing a one-line progress bar on stderr (when `show`).
fn with_terminal_progress<T>(progress: &Progress, show: bool, work: impl FnOnce() -> T) -> T {
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    if !show {
        return work();
    }
    let finished = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut last = String::new();
            while !finished.load(Ordering::Relaxed) {
                let line = progress_line(progress);
                if line != last {
                    eprint!("\r{line:<78}");
                    let _ = std::io::stderr().flush();
                    last = line;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprint!("\r{:<78}\r", "");
        });
        let out = work();
        finished.store(true, Ordering::Relaxed);
        out
    })
}

fn progress_line(progress: &Progress) -> String {
    let p = progress.snapshot();
    let label = match p.stage {
        Stage::Idle => "Starting",
        Stage::Discover => "Scanning files",
        Stage::Read => "Reading files",
        Stage::Parse => "Parsing",
        Stage::Store => "Storing symbols",
        Stage::Resolve => "Resolving references",
        Stage::Link => "Linking project",
        Stage::Save => "Saving",
        Stage::Done => "Done",
    };
    const WIDTH: usize = 24;
    let filled = ((p.percent / 100.0) * WIDTH as f32).round() as usize;
    let bar = format!("{}{}", "█".repeat(filled.min(WIDTH)), "░".repeat(WIDTH - filled.min(WIDTH)));
    let count = match (p.stage, p.total) {
        (Stage::Discover, _) => format!(" {} found", p.done),
        (_, 0) => String::new(),
        (_, t) => format!(" {}/{t}", p.done.min(t)),
    };
    format!("{bar} {:>3.0}%  {label}{count}", p.percent)
}

fn dump(n: tree_sitter::Node, src: &str, depth: usize, field: Option<&str>) {
    let txt = if n.child_count() == 0 { format!(" {:?}", &src[n.byte_range()]) } else { String::new() };
    println!(
        "{}{}{}{}  [{}-{}]",
        "  ".repeat(depth),
        field.map(|f| format!("{f}: ")).unwrap_or_default(),
        n.kind(),
        txt,
        n.start_position().row + 1,
        n.end_position().row + 1
    );
    let mut c = n.walk();
    for (i, ch) in n.children(&mut c).enumerate() {
        dump(ch, src, depth + 1, n.field_name_for_child(i as u32));
    }
}

fn open_browser(url: &str) {
    let res = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = res {
        eprintln!("(could not open a browser automatically: {e})");
    }
}
