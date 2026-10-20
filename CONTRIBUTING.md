# Contributing to CodeGraph

Thank you for your interest in contributing to **CodeGraph**! We welcome contributions of all kinds: new language parsers, semantic resolvers, MCP analytical tools, performance optimizations, bug fixes, and documentation improvements.

---

## Workspace Architecture

CodeGraph is structured as a modular Rust workspace consisting of six crates:

| Crate | Path | Responsibility |
|---|---|---|
| **`codegraph-core`** | `crates/core` | File discovery (`ignore`), parallel Blake3 content hashing (`rayon`), embedded Tree-sitter parsers, AST fact extraction, file skeletons, token budget estimation, and AST-level structural diffing. |
| **`codegraph-semantic`** | `crates/semantic` | Cross-file semantic resolution, multi-tiered confidence scoring (`exact`, `inferred`, `unresolved`), DI resolution, fullstack route matching, error flow, data model / ERD extraction, and architectural boundary rules. |
| **`codegraph-graph`** | `crates/graph` | Embedded SQLite engine (`WAL` mode + `FTS5` trigram tokenization), schema migrations (`PRAGMA user_version`), incremental indexing pipeline, and cascading cache invalidation. |
| **`codegraph-mcp`** | `crates/mcp` | Model Context Protocol (MCP) server implementation over stdio (JSON-RPC 2.0), runtime engine, file change watcher (`notify`), targeted freshness checker, and 28 analytical tools. |
| **`codegraph-ui`** | `crates/ui` | Embedded Axum HTTP server hosting the Cytoscape.js interactive graph explorer SPA compiled into the binary via `rust-embed`. |
| **`codegraph-cli`** | `crates/cli` | Command-line interface (`codegraph index`, `mcp`, `ui`, `query`, `tools`, `status`, `dump-ast`, `facts`). |

Outside the workspace, `skills/codegraph/SKILL.md` is the agent skill (Claude Code, Codex, Antigravity) that teaches coding agents to read supported files through CodeGraph. It isn't compiled; keep it in sync with the tool registry and supported extensions.

---

## Prerequisites

- **Rust**: Version 1.80+ with Cargo (`rustup update stable`).
- **C/C++ Compiler**: Required for compiling embedded Tree-sitter C grammars (`gcc`, `clang`, or MSVC on Windows).
- **Git**: For version control tests (`get_structural_diff` / `get_changed_subgraph`).

---

## Development Setup

1. **Clone the repository:**
   ```bash
   git clone https://github.com/codegraph/codegraph.git
   cd codegraph
   ```

2. **Build the workspace:**
   ```bash
   cargo build
   ```

3. **Run the test suite:**
   ```bash
   cargo test --workspace
   ```
   Tests include syntax extraction fixtures, incremental indexing verification, MCP protocol compliance, and end-to-end runs of all 28 MCP analytical tools.

4. **Verify formatting and clippy:**
   ```bash
   cargo fmt --check
   cargo clippy --workspace --all-targets
   ```

---

## Developing the Web Graph Explorer

The Graph Explorer UI assets reside in `crates/ui/assets/`. In release mode, these assets are compiled directly into the binary via `rust-embed`.

During local UI development, you can point the environment variable `CODEGRAPH_UI_DEV` to the assets directory so that changes to HTML, CSS, or JS reload immediately without recompiling Rust:

```bash
# Windows PowerShell
$env:CODEGRAPH_UI_DEV = "crates/ui/assets"
cargo run -p codegraph-cli -- ui fixtures/fullstack

# Linux / macOS
CODEGRAPH_UI_DEV=crates/ui/assets cargo run -p codegraph-cli -- ui fixtures/fullstack
```

---

## How to Add a New MCP Tool

1. Create or open the appropriate module in `crates/mcp/src/tools/` (e.g., `discovery.rs`, `navigation.rs`, `tokens.rs`, etc.).
2. Define the input parameters struct deriving `serde::Deserialize` and `schemars::JsonSchema`.
3. Implement the tool function returning `crate::tools::ToolResult` (`Result<Out, ToolError>`).
4. Register the tool with the `tool!` macro in `crates/mcp/src/tools/mod.rs` inside `pub fn registry()`.
5. Add unit and integration tests in `crates/mcp/tests/tools.rs`.
6. Document the tool in `README.md` and, if agents should reach for it, add it to the tables in `skills/codegraph/SKILL.md`.

---

## How to Add Support for a New Language

1. Add the corresponding Tree-sitter grammar dependency to `crates/core/Cargo.toml`.
2. Register the language enum variant in `crates/core/src/lang.rs`.
3. Implement an extractor in `crates/core/src/extract/<language>.rs` extracting:
   - Symbols (classes, interfaces, methods, functions, variables, constants, fields).
   - References (calls, constructors, reads, writes, throws, catches).
   - Imports and exports.
   - Framework routes, DI registrations, and configuration accesses where applicable.
4. Add fixture files and extraction unit tests in `crates/core/tests/extraction.rs`.
5. List the new extensions in the README's *Supported Languages* table and in both the `description` and *Supported files* table of `skills/codegraph/SKILL.md`, so agents know to read those files through the graph.

---

## Pull Request Guidelines

- Ensure `cargo test --workspace` passes cleanly.
- Ensure `cargo fmt --check` succeeds.
- Keep commits focused and provide clear, descriptive commit messages.
- If introducing user-visible features or CLI commands, update the documentation in `README.md`.
