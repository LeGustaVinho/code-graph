---
name: codegraph
description: Read and navigate source code through the CodeGraph semantic index instead of opening whole files. Use whenever you need to read, explore, search or understand files CodeGraph supports — C# (.cs), TypeScript (.ts/.tsx/.mts/.cts, not .d.ts), JavaScript (.js/.jsx/.mjs/.cjs, not .min.js), Vue / Svelte components (.vue/.svelte), Python (.py/.pyi), Rust (.rs), Prisma (.prisma) and config files (appsettings*.json, local.settings.json, .env, .env.*) — or to find symbols, callers/callees, usages, implementations, DI bindings, routes, data models, config keys, tests for a symbol, or the impact of a change. Prefer it over Read/cat/grep for these files.
---

# CodeGraph

**Consultas econômicas:** Comece com max_token_budget=500 e depth=1; aumente para 2 quando precisar de mais relações. get_tests_for_symbol prioriza testes ligados diretamente (depth padrão 2, limit padrão 12); arquivos com nomes semelhantes são apenas candidatos. get_config_matrix aceita path para limitar o arquivo que lê a configuração e filter para restringir nomes de chaves, além de include_unread para detalhar definições sem usos indexados. get_call_hierarchy agrupa sites por origem; show_sites=true inclui os trechos completos das chamadas e aumenta o consumo. get_file_skeleton filtra declarações por export em módulos ES identificáveis, mantendo as regras das demais linguagens. Consulte o schema antes de usar formatos específicos como JSON.

**Pare quando houver evidência suficiente:** Para listas de chamadas use primeiro get_call_hierarchy depth=1 com sites; para envs, get_config_matrix com path; para testes, get_tests_for_symbol seguido do outline do teste mais relevante; para fluxos HTTP, trace_fullstack_flow. Se as saídas já contêm os dados pedidos com arquivo:linha, responda sem repetir a mesma leitura via get_symbol_body, outline ou grep. Expanda o corpo de um símbolo ou use leitura crua só quando faltar uma condição/SQL essencial, houver incerteza de edge, ou a resposta anterior for contraditória/incompleta. Não reduza a cobertura para caber no orçamento: aumente o budget quando necessário.

**Trecho de um corpo longo:** \`get_symbol_body\` aceita \`symbol_id\`, \`start_line\` e \`end_line\` (ambos obrigatórios ao selecionar, linhas **absolutas do arquivo**, inclusivas e dentro do símbolo). Ex.: \`{"symbol_id":"Service.process","start_line":35,"end_line":50}\` devolve somente essas 16 linhas com numeração e indentação originais. Sem esses parâmetros, mantém o comportamento usual; \`max_lines\` continua limitando o prefixo do trecho retornado.

A semantic index of the repo (symbols, calls, types, DI, routes, ORM models, config keys) with exact `file:line` evidence. For the file types above, ask it first and open raw text only for lines you need.

**Call**: MCP tools `mcp__codegraph__*` if registered, else `codegraph query <tool> --root <repo> --args '<json>'` (binary: `codegraph` on PATH, else `<codegraph-repo>/target/release/codegraph[.exe]`; put the absolute path here when installing). Bash: single-quote the JSON. PowerShell 5.1: `--args '{\"query\":\"Order\"}'`. Changed files are re-indexed automatically. All tools take `max_token_budget`, `format:"json"`; `codegraph tools --json` lists schemas.

**Symbols**: `Name`, `Type.member`, `outer.inner` (nested functions), `file.ts:120` (innermost at that line) or `sym_…`; an ambiguous name lists the candidates. A type stands for its members; a route symbol (`"POST /v1/orders"`) spans its whole registration. `file_path` takes a unique suffix.

**Workflow**: map a file with one call — `get_file_skeleton` (public API) *or* `get_file_outline` (all members, complexity, side-effect tags); read just what matters with `get_symbol_body` (`symbol_id`), several at once in `batch_query` (`operations:[{"tool":…,"args":{…}}]`); follow relationships with the tools below; before editing, read only the lines you change. Results are complete for `exact` edges (caller lists include every site), so don't re-grep them; verify only `inferred`/`unresolved` hints or answers that look wrong, and say so.

| Need | Tool (key args) |
|---|---|
| Find a symbol / a literal | `search_universal` (`query`, `kind?`) · `find_literal` (`query`, `exact?`) |
| Variable reads & writes | `find_variable_or_const` (`name`) |
| Callers / callees, every site (`@12,15`; callers at depth 1 print the call lines) | `get_call_hierarchy` (`symbol_id`, `direction`, `depth`) |
| Supertypes / implementations · public API | `get_type_hierarchy` · `get_type_contract` |
| Path A→B · route ↔ frontend callers → handler → DB | `trace_execution_path` · `trace_fullstack_flow` (`api_route_or_call:"POST /v1/orders"`, `depth?`) |
| DI binding · exception throw/catch · mutations | `resolve_dependency_injection` · `trace_error_flow` · `get_state_mutations` |
| By side effect (`DB_WRITE`, `NETWORK_IO`, …) · entry points · ORM models | `find_by_capability` · `get_entrypoints` · `get_data_model` |
| Env/config keys by reading function | `get_config_matrix` (`path?`, `filter?`) |
| Tests for a symbol (closest first) | `get_tests_for_symbol` (`symbol_id`, `depth?`) |
| Change impact | `get_impact_radius`, `simulate_signature_change`, `get_changed_subgraph`, `get_structural_diff` |

Lines read `m OrderService.checkout src/services.ts:20-29 | checkout(...)`. If CodeGraph errors or is missing, read files normally and say so.
