---
name: codegraph
description: Navigate indexed C#, TypeScript/JavaScript, Vue/Svelte, Python, Rust and Prisma code with CodeGraph.
---

Use `mcp__codegraph__*` or `codegraph query TOOL --root REPO --args JSON`. Pass JSON as one safely quoted argument. Use the executable in CodeGraph's `target/release` if needed. List or inspect schemas with `codegraph tools [TOOL] --json`.

Start from a known path or symbol. Use `path:Name` in `symbol_id` for local declarations or resolved imports. Try `max_token_budget:500` and `depth:1` where supported. Batch independent calls with `batch_query`. Expand only missing evidence. Cite file:line. Count failures and fallbacks.

- Orient with `get_project_overview(path?)` or `get_entrypoints`. Find indexed paths by literal name or suffix with `find_files(query,path?)`.
- Locate declarations with `search_universal(query,exact:true,kind?)`. Omit `exact` for fuzzy search.
- Find text and comments with `search_source(query,path?,whole_word?)`, or `queries:[...]` for one scope. Both searches support `group_by_file:true`.
- Read APIs with `get_file_skeleton` (`include_private:true` for private members) or `get_file_outline` (`include_signatures:true`). Read selected statements with `get_flow_evidence`. Outline `names` and flow `members` match exactly. Use `filter` instead for substrings.
- Verify omitted logic with `get_symbol_body(symbol_id)`. Ranges require `start_line` and `end_line` (1-based inclusive, within the symbol). Use `file_path` with both bounds to cross declarations.
- Navigate with `get_call_hierarchy` (`direction`, `show_sites:true`), `get_type_contract`, `get_type_hierarchy` and `get_tests_for_symbol`. Disambiguate overloads by signature, `sym_...` ID or `Name/<arity>` where supported. Exact edges can still be overload-ambiguous.
- Trace HTTP registration, parsers and calls with `trace_fullstack_flow(api_route_or_call,route_evidence:true)`. Verify backend behavior at its source. Use `resolve_dependency_injection`, `get_state_mutations`, `trace_error_flow` or `get_config_matrix` as needed.

Zero text matches cover indexed, readable files only. A verified empty source scope has no supported files and needs no fallback. Verify missing paths, ignored files and index gaps before claiming absence. Use targeted conventional reads for verification or when CodeGraph is unavailable. Disclose fallbacks.

Honor requested file types, including in fallbacks. Keep C# code-only references in `.cs`. Unity Inspector and dynamic bindings may evade static edges. Inspect `.prefab`/`.unity` only on request.
