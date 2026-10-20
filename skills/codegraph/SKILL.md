---
name: codegraph
description: Read and navigate source code through the CodeGraph semantic index instead of opening whole files. Use whenever you need to read, explore, search or understand files CodeGraph supports — C# (.cs), TypeScript (.ts/.tsx/.mts/.cts, not .d.ts), JavaScript (.js/.jsx/.mjs/.cjs, not .min.js), Python (.py/.pyi), Rust (.rs), Prisma (.prisma) and config files (appsettings*.json, local.settings.json, .env, .env.*) — or to find symbols, callers/callees, usages, implementations, DI bindings, routes, data models, config keys, tests for a symbol, or the impact of a change. Prefer it over Read/cat/grep for these files.
---

# CodeGraph: read code through the graph

CodeGraph parses a repository into a semantic graph (symbols, calls, types, DI, routes, ORM models, config keys) and answers questions with exact file:line evidence. For supported files, ask the graph first and open raw text only for the lines you actually need. This saves most of the tokens a full-file read costs and gives you relationships grep cannot see.

## Supported files

| Language | Files |
|---|---|
| C# (incl. Unity) | `.cs` |
| TypeScript | `.ts` `.tsx` `.mts` `.cts` — **not** `.d.ts` |
| JavaScript | `.js` `.jsx` `.mjs` `.cjs` — **not** `.min.js` |
| Python | `.py` `.pyi` |
| Rust | `.rs` |
| Prisma | `.prisma` |
| Config (keys only) | `appsettings*.json`, `local.settings.json`, `.env`, `.env.*` |

Anything else (Markdown, YAML, other JSON, `.d.ts`, HTML, CSS, shaders, scenes, Unity assets…) — read it normally.

## How to call it

Use whichever is available, in this order:

1. **MCP tools** named `codegraph` / `mcp__codegraph__*` (if the MCP server is registered). Call the tool directly with the same arguments shown below.
2. **CLI**: `codegraph query <tool> --root <project-root> --args '<json>'`

Binary: `codegraph` if it is on PATH, otherwise the build output of a CodeGraph checkout
(`<codegraph-repo>/target/release/codegraph`, `codegraph.exe` on Windows). When installing this
skill, replace this line with the absolute path of your binary if it isn't on PATH.

- `--root` is the repository root (where `.git` is). The first query indexes the project into `<root>/.codegraph/graph.db` (and adds `.codegraph/` to `.gitignore`); later queries re-index changed files automatically, so results reflect your edits. Run `codegraph index <root>` up front on a big repo if you want to see progress; `codegraph status <root>` shows index stats.
- `codegraph tools --json` prints every tool's full input schema.
- **Quoting.** In bash/sh use single quotes around the JSON: `--args '{"query":"Order"}'`. In **Windows PowerShell 5.1**, native-exe arguments lose their inner double quotes, so escape them: `--args '{\"query\":\"Order\"}'` (the unescaped form fails with "--args must be valid JSON"). Prefer the bash shell when one exists.
- Every tool accepts `max_token_budget` (truncates with an explicit `[TRUNCATED …]` marker) and `format` (`"json"` for structured output; default is compact text).

### Symbol identifiers

Wherever a tool takes `symbol_id` / `type_id` / `source` / `target`, any of these work: simple name `OrderService`, member `OrderService.CancelOrderAsync`, qualified `App.Services.OrderService`, coordinate `Services/OrderService.cs:26`, or an id `sym_…`. If a simple name is ambiguous, use the member form or a coordinate. `file_path` is relative to the root, and a unique suffix is accepted (`services.rs`).

## Reading workflow (instead of opening a whole file)

1. **Map the file** — `get_file_outline` (kinds, line ranges, complexity `ccN`, capability tags like `[DB_WRITE,THROWS]`) or `get_file_skeleton` (public signatures only, cheapest).
2. **Read only what matters** — `get_symbol_body` for the specific method/class (`strip_comments`, `max_lines` available).
3. **Follow relationships via the graph**, not by opening more files: callers/callees, type hierarchy, DI, etc. (table below).
4. **Before editing**, read the exact line range from step 1 with your normal file reader (offset/limit) so the editor has the current text. Don't re-read the whole file.
5. Batch independent lookups with `batch_query` (≤ 20 operations, one roundtrip).

Reading the full file is still right when: the file is small (≈ < 100 lines) and you need all of it; you need comments, ordering or formatting around a change; the file type isn't supported; or the graph answer looks wrong/incomplete (say so, then verify with raw text). Treat `inferred`/`unresolved` edges as hints and confirm them in source; `exact` edges are backed by evidence.

## Use instead of grep

| You want | Tool | Key args |
|---|---|---|
| Find a class/function/variable by name | `search_universal` | `query`, `kind?`, `limit?` |
| Find a string/number literal (route, error text, SQL, key) | `find_literal` | `query`, `exact?`, `literal_type?` |
| Where a variable/field/const is read & written | `find_variable_or_const` | `name`, `scope?` |
| Who calls X / what X calls | `get_call_hierarchy` | `symbol_id`, `direction` = callers\|callees\|both, `depth` |
| Base types / implementations | `get_type_hierarchy` | `symbol_id`, `direction?` |
| Public API of a type, no bodies | `get_type_contract` | `type_id` |
| Path from A to B | `trace_execution_path` | `source`, `target`, `max_depth?` |
| Frontend call → backend route → services → DB | `trace_fullstack_flow` | `api_route_or_call` (e.g. `"GET /api/orders"`) |
| Which implementation DI injects | `resolve_dependency_injection` | `interface_or_token` |
| Where an exception is thrown / caught | `trace_error_flow` | `exception_name_or_message` |
| State a function mutates | `get_state_mutations` | `symbol_id`, `include_callees?` |
| Functions by side effect | `find_by_capability` | `tags` e.g. `["DB_WRITE"]`, `match_any?` |
| Entry points (routes, main, CLI, handlers, Unity messages) | `get_entrypoints` | `kind?` |
| ORM / Prisma models | `get_data_model` | `model_name?` (omit to list) |
| Config/env keys read vs defined | `get_config_matrix` | `filter?` |
| Small Mermaid call diagram | `get_flow_diagram` | `symbols` (array) |

Capability tags: `DB_READ DB_WRITE NETWORK_IO FILE_IO LOCAL_STORAGE ASYNC AUTH_REQUIRED CACHE CONFIG_READ THROWS GLOBAL_STATE LOGGING EVENT_EMIT`.

## Before and after changing code

| Question | Tool |
|---|---|
| What breaks if I change X? | `get_impact_radius` (`symbol_id`, `depth?`) |
| Which call sites break if the parameter count changes? | `simulate_signature_change` (`symbol_id`, `new_param_count`, `new_required_count?`) |
| Which tests cover X? | `get_tests_for_symbol` |
| What did my diff touch, and what depends on it? | `get_changed_subgraph` (`git_ref?`, default HEAD incl. uncommitted) |
| Symbol-level diff ignoring formatting | `get_structural_diff` (`git_ref?`, `path?`) |
| Layering rule allowed? | `validate_dependency_rule` (`source_module`, `target_module`) |
| Risky / complex functions, dead code | `get_complexity_hotspots`, `find_orphaned_symbols` |

## Examples (bash)

```bash
CG=codegraph   # or: CG=/path/to/codegraph/target/release/codegraph
ROOT=/path/to/repo

$CG query get_file_outline   --root "$ROOT" --args '{"file_path":"src/services.rs"}'
$CG query get_symbol_body    --root "$ROOT" --args '{"symbol_id":"OrderService.checkout"}'
$CG query search_universal   --root "$ROOT" --args '{"query":"OrderRepo","limit":10}'
$CG query get_call_hierarchy --root "$ROOT" --args '{"symbol_id":"Order.new","direction":"callers","depth":2}'
$CG query batch_query        --root "$ROOT" --args '{"operations":[
  {"tool":"get_file_skeleton","args":{"file_path":"src/models.rs"}},
  {"tool":"get_tests_for_symbol","args":{"symbol_id":"Order.total"}}]}'
```

PowerShell equivalent of one call:

```powershell
& codegraph query get_symbol_body --root D:\repo --args '{\"symbol_id\":\"OrderService.checkout\"}'
```

Output lines look like `m OrderService.checkout src/services.rs:20-29 | pub fn checkout(...)` — kind, name, `file:start-end`, signature. Use those ranges for targeted reads and edits.

## Notes

- If the binary lives in a CodeGraph checkout's `target/release`, don't query while that repo is being rebuilt (`cargo build` can't replace a running exe); stop any `codegraph ui`/`mcp` processes before rebuilding it.
- If a query errors or the binary is missing, fall back to normal file reading and tell the user CodeGraph was unavailable.
