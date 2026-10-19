//! `codegraph` — deep semantic code indexer, MCP server and graph explorer.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codegraph_core::lang::{parse, Language};
use codegraph_graph::{index_project, IndexOptions};
use codegraph_mcp::Engine;

#[derive(Parser)]
#[command(name = "codegraph", version, about = "Deep semantic code graph: indexer, MCP server (stdio) and local graph explorer")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
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
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).with_target(false).try_init();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Index { path, rebuild, json } => {
            init_logging("warn");
            let root = root_or_cwd(path)?;
            let mut conn = codegraph_graph::open(&root)?;
            let stats = index_project(&root, &mut conn, IndexOptions { rebuild })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&stats)?);
            } else {
                let g = codegraph_graph::query::stats(&conn)?;
                println!("Indexed {} in {} ms", root.display(), stats.duration_ms);
                println!(
                    "  files: {} total, {} parsed, {} unchanged, {} removed, {} errors",
                    stats.files_total, stats.files_parsed, stats.files_unchanged, stats.files_removed, stats.parse_errors
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
            eprintln!("Indexing {} …", root.display());
            let engine = Engine::open(&root)?;
            engine.start_watcher();
            let addr: SocketAddr = format!("{host}:{port}").parse().context("invalid host/port")?;
            let url = format!("http://{}:{port}", if host == "0.0.0.0" { "localhost" } else { host.as_str() });
            eprintln!("CodeGraph Explorer: {url}");
            if !no_open {
                open_browser(&url);
            }
            let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
            rt.block_on(codegraph_ui::serve(engine, addr))?;
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
