---
name: codegraph
description: Navigate C#, TypeScript/JS/TSX/JSX, Vue/Svelte, Python, Rust, Prisma and configuration with indexed code symbols, flows, callers, tests, routes and types.
---

# CodeGraph navigation

Use installed `mcp__codegraph__*` tools, or CLI: `codegraph query <tool> --root <repo> --args <json>` (Windows binary: `D:\Entidades\Gustavo\Projects\CodeGraph\target\release\codegraph.exe`). Pass JSON as a separate safely quoted argument. Output defaults to compact. Some tools also accept JSON output. For a CLI tool's parameters, run `codegraph tools TOOL --json`.

**Start with the smallest sufficient evidence.** Pass an already-known symbol or file straight to its tool; discovery is unnecessary. Usually set `max_token_budget:500`, `depth:1`, then expand only omitted evidence. Stop when file:line evidence answers the question. Count fallbacks and failed queries.

| Goal | Tool |
| --- | --- |
| Project orientation | `get_project_overview(limit?,max_token_budget?)` for indexed languages, code directories and sampled entrypoints |
| Exact declaration | `search_universal(query,exact:true,kind?)`; omit `exact` for fuzzy discovery. Use `group_by_file:true` for multiple hits in the same file |
| Text uses, comments, scoped absence | `search_source(query,path?,whole_word?,group_by_file?)` |
| File API / named members | `get_file_skeleton(file_path,include_private?)` or `get_file_outline(file_path,names:[...],include_signatures:true?)` |
| Type members / implementations | `get_type_contract(type_id)`, `get_type_hierarchy(symbol_id,direction?)` |
| Method flow / guards / mutations | `get_flow_evidence(symbol_id,members:[...]?,max_lines?)` |
| Callers / callees | `get_call_hierarchy(symbol_id,direction:"callers"|"callees",depth:1,show_sites:true)` |
| Tests / config | `get_tests_for_symbol(symbol_id)`, `get_config_matrix(filter?,path?)` |
| HTTP registration, body wiring, direct calls | `trace_fullstack_flow(api_route_or_call,route_evidence:true)` |
| Fullstack flows / DI / side effects | `trace_fullstack_flow`, `resolve_dependency_injection`, `get_state_mutations`, `trace_error_flow` |

`names`/`members` select multiple **exact** names in one query; `filter` is a substring alternative. `include_signatures:true` adds signatures to selected outline members. A type flow is an incomplete selection: inspect missing branches with `get_symbol_body(symbol_id,start_line?,end_line?)` (absolute inclusive bounds), when needed. Public skeletons omit private methods unless requested. For overloaded methods use an exact signature, `sym_...` ID or `Name/<arity>` where supported. An `exact` call edge can still be overload-ambiguous; Unity Inspector and dynamic bindings may lack static edges. **Honor the requested file types:** C# code-only reference searches must remain in `.cs` files, including conventional fallbacks; inspect Unity `.prefab`/`.unity` bindings only when explicitly requested.

A zero text match covers only indexed, readable files. A `verified empty source scope` means the directory contains no supported source files and needs no fallback. Missing paths, ignored source files and other incomplete index coverage still require conventional verification before claiming absence. Never infer server behavior from a client call alone. Other tools: `batch_query`, `get_entrypoints`, `find_literal`, `get_data_model`, `get_impact_radius`, `get_structural_diff`. If CodeGraph is unavailable, use targeted conventional reads and disclose the fallback.
