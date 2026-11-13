---
name: codegraph
description: Navigate indexed C#, TypeScript/JavaScript, Vue/Svelte, Python, Rust and Prisma code with CodeGraph.
---

Use CodeGraph MCP or `codegraph query TOOL --root REPO --args JSON` (one quoted JSON argument, binary in CodeGraph `target/release`). Start from known paths or symbols, use `max_token_budget:500` and `depth:1` where supported, and cite file:line.

- Orient with `get_project_overview(path?)`. Find indexed filenames or suffixes with `find_files(query,path?)`.
- Find declarations with `search_universal(query,exact:true,kind?)` or omit `exact` for fuzzy search. For text and comments use `search_source(query,path?,whole_word?)` or `queries:[...]` in one scope. Both support `group_by_file:true`.
- Use `get_file_outline(file_path,names?,include_signatures?)` or `get_flow_evidence(symbol_id,members?)`. `names`/`members` match exactly. Use `filter` instead for substrings.
- Use `get_symbol_body(symbol_id)`. For ranges, both `start_line`/`end_line` are inclusive absolute lines within the symbol. With both bounds, use `file_path` instead to cross declarations.
- Use `path:Name` for scoped symbols or imports. Disambiguate by signature, `sym_...` ID or `Name/<arity>` where supported. Use `get_call_hierarchy(symbol_id,direction,show_sites:true)` and `get_tests_for_symbol(symbol_id)`.
- Use `trace_fullstack_flow(api_route_or_call,route_evidence:true)` for HTTP evidence. Verify backend behavior in its source.

Use `batch_query` for independent reads. For other workflows read [references/tools.md](references/tools.md). Fetch only the needed schema with `codegraph tools TOOL --json` or compact MCP `get_tool_schema(name)` and run unlisted tools via `batch_query`.

Zero text hits cover indexed readable files only. A verified empty source scope has no supported source. Verify gaps with targeted reads. Count failures and disclose fallbacks. Honor requested extensions, including `.cs` for C# code-only tasks. Static edges can miss dynamic and Unity Inspector bindings or leave overloads ambiguous. Inspect `.prefab`/`.unity` only on request.
