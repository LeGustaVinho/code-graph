# CodeGraph — MCP & Deep Semantic Indexer (Rust)

Indexador de código de alto desempenho que constrói um **grafo semântico** do repositório
(C#, TypeScript/TSX, JavaScript/JSX, Python, schemas Prisma e arquivos de configuração),
com **níveis formais de confiança** em cada relação, e o expõe como:

- **Servidor MCP** (JSON-RPC 2.0 sobre stdio) com **28 ferramentas analíticas** e orçamento estrito de tokens;
- **Graph Explorer** local (Axum + Cytoscape.js embutidos) em `http://localhost:4321`;
- **CLI** (`codegraph index | mcp | ui | query | tools | status`).

Tudo em um **binário único** (~20 MB) sem dependências externas: gramáticas Tree-sitter,
SQLite (WAL + FTS5 trigram) e a interface web estão compilados dentro dele.

## Início rápido

```bash
cargo build --release
```

```bash
./target/release/codegraph index caminho/do/projeto
```

```bash
./target/release/codegraph ui caminho/do/projeto
```

```bash
./target/release/codegraph query search_universal --root caminho/do/projeto --args '{"query":"OrderService"}'
```

O índice fica em `<projeto>/.codegraph/graph.db` (um `.gitignore` é criado automaticamente).

## Conectando como servidor MCP

**Claude Code**

```bash
claude mcp add codegraph -- /caminho/para/codegraph mcp --root /caminho/do/projeto
```

**Claude Desktop / Cursor / Antigravity** (`claude_desktop_config.json`, `.cursor/mcp.json`, …):

```json
{
  "mcpServers": {
    "codegraph": {
      "command": "C:/tools/codegraph.exe",
      "args": ["mcp", "--root", "D:/repos/minha-app"]
    }
  }
}
```

A raiz também pode vir de `CODEGRAPH_ROOT`. Logs vão para stderr (`CODEGRAPH_LOG=debug`).

## Níveis de confiança

Toda aresta tem `confidence` + `evidence` + `resolver`:

| Nível | Quando | Exemplo de evidência |
| --- | --- | --- |
| `exact` | import direto, tipo declarado/inferido, classe envolvente, registro de DI | `DI scoped registration IOrderService -> OrderService in Program.cs:12` |
| `inferred` | convenção de nomes, rota HTTP idêntica, implementação única | `route match 'POST /api/orders/${id}/cancel' ↔ POST /api/Orders/{id}/cancel` |
| `unresolved` | alvo ambíguo/dinâmico (`target_id` nulo, `unresolved_symbol` preenchido) | `3 potential implementations of IHandler.Execute` |

Saída compacta padrão: `src/auth.ts:10 AuthStore.login -> src/utils/hash.ts:3 hash [exact: direct import from './hash']`.

## Ferramentas MCP (28)

| Grupo | Ferramentas |
| --- | --- |
| Economia de tokens | `batch_query`, `get_file_skeleton`, `get_structural_diff`, `simulate_signature_change`, `get_flow_diagram` |
| Descoberta | `search_universal`, `find_literal`, `find_variable_or_const` |
| Navegação | `trace_execution_path`, `trace_fullstack_flow`, `resolve_dependency_injection`, `get_call_hierarchy`, `get_type_hierarchy`, `get_impact_radius` |
| Comportamento | `get_state_mutations`, `trace_error_flow`, `find_by_capability`, `get_entrypoints`, `find_orphaned_symbols` |
| Dados & extração | `get_data_model`, `get_config_matrix`, `get_type_contract`, `get_symbol_body`, `get_file_outline` |
| Governança & Git | `get_complexity_hotspots`, `validate_dependency_rule`, `get_tests_for_symbol`, `get_changed_subgraph` |

Todas aceitam `max_token_budget`; quando o limite é atingido a resposta termina com
`[TRUNCATED: Showing 4 of 19 results to fit 150 token budget (142 tokens used)]`.
Argumentos são validados estritamente (campos desconhecidos são rejeitados). Símbolos podem ser
passados por id (`sym_…`), nome, `Tipo.membro`, nome qualificado ou `arquivo:linha`.
`codegraph tools --json` imprime os schemas completos.

## Arquitetura

```
crates/
  core      descoberta (ignore), hashing Blake3 paralelo (Rayon), Tree-sitter, extração sintática,
            esqueletos/outlines, diff estrutural de AST, orçamento de tokens
  semantic  resolução multinível (exact/inferred/unresolved): chamadas, herança, imports,
            DI (.NET/NestJS/FastAPI), rotas fullstack, fluxo de erros, testes, literais, ERD, regras de arquitetura
  graph     SQLite versionado (PRAGMA user_version, WAL, FTS5 trigram) + indexador incremental
  mcp       servidor MCP (Tokio, stdio) e as 28 ferramentas; freshness check antes de cada chamada
  ui        servidor Axum + SPA Cytoscape.js embutida (rust-embed)
  cli       binário `codegraph`
```

**Indexação incremental:** filtro por `mtime`/tamanho → hash Blake3 apenas dos candidatos →
parse paralelo só dos arquivos com conteúdo alterado → invalidação em cascata (arestas cujo alvo
estava no arquivo alterado e arquivos que referenciam nomes definidos/removidos) → resolução
semântica só dos arquivos afetados → relações derivadas globais (DI, HTTP, testes, CAUGHT_BY, literais, ERD).

**Freshness:** `codegraph mcp` e `codegraph ui` observam o disco (notify); antes de responder,
arquivos sujos e arquivos citados nos argumentos são reindexados.

## Regras de arquitetura

`validate_dependency_rule` usa `.codegraph/architecture.toml` (ou a tabela `[architecture]` de `codegraph.toml`):

```toml
[[layers]]
name = "domain"
paths = ["src/Domain/**"]

[[layers]]
name = "infrastructure"
paths = ["src/Infrastructure/**"]

[[rules]]
from = "domain"
deny = ["infrastructure"]
```

Sem configuração é usada uma convenção em camadas (presentation → application → domain; infrastructure → domain).

## Desempenho medido (release, Windows, 8.700 arquivos / 50 mil nós / 138 mil arestas)

| Métrica | Resultado |
| --- | --- |
| Indexação completa | 5,9 s |
| Reindexação sem alterações | 16 ms |
| Reindexação incremental de 1 arquivo | ~1,4 s (600 arquivos afetados pela cascata) |
| `search_universal` (ida e volta MCP) | ~1,4–2,6 ms |
| Memória do servidor MCP (RSS) | 14–17 MB |

## Testes

```bash
cargo test --workspace
```

Cobrem extração por linguagem, IDs/hashes estáveis, consistência incremental, integridade
referencial, níveis de confiança com evidência, orçamento de tokens, freshness check,
protocolo MCP e as 28 ferramentas sobre o projeto de exemplo em `fixtures/fullstack`
(React + ASP.NET Core + NestJS + FastAPI + Prisma).
