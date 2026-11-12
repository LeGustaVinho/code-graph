# Benchmark CodeGraph em MCP, CLI e leitura por agentes

Em 12/11/2026, dois agentes com o mesmo modelo responderam independentemente às mesmas sete tarefas. Cada agente fez uma execução, na mesma ordem, sobre cópias dos mesmos commits. O braço convencional usou listagem, busca textual e leitura por intervalo. O braço CodeGraph usou a CLI do commit `26acb4e` e leu a skill uma vez. Um terceiro agente conferiu os fatos e as coordenadas nas fontes.

## Ensaio com agentes

O registro incluiu toda saída de leitura, erros e a skill. A contagem de tokens usa `tiktoken` 0.14.0 com `o200k_base`. A coluna CodeGraph contabiliza a skill em S0. A métrica de argumentos soma os tokens da representação dos pedidos registrada pelo wrapper. O tempo somado mede somente as operações instrumentadas.

| Cenário | Convencional | CodeGraph |
| --- | ---: | ---: |
| S0 · Leitura da skill | 0 | 748 |
| S1 · Orientação inicial em app e testes C# | 623 | 393 |
| S2 · Call sites diretos de uma operação C# | 556 | 284 |
| S3 · Teste e dois valores esperados de formatação | 398 | 346 |
| S4 · Responsabilidades de três diretórios de um pacote Unity | 883 | 542 |
| S5 · Cadeia de callbacks de renderização e controle por frame | 2.897 | 2.049 |
| S6 · Clientes, API, bootstrap e contratos de um monorepo | 3.284 | 1.565 |
| S7 · Registro, autenticação, schema e serviço de uma rota | 2.025 | 2.542 |
| **Total de tokens de saída** | **10.666** | **8.469** |
| **Total com argumentos** | **11.539** | **10.183** |
| Operações | 38 | 28 |
| Tempo somado das operações | 0,258 s | 2,507 s |
| Tarefas factualmente corretas | 7 de 7 | 7 de 7 |

A economia agregada foi de **20,60% na saída** e **11,75% com argumentos**. A média das economias por tarefa foi de **27,66%**, com S0 contabilizado separadamente. S7 consumiu **25,53% mais tokens de saída** com CodeGraph. O braço CodeGraph fez 27 chamadas à CLI e uma leitura da skill. O convencional fez 19 leituras por intervalo, 17 buscas e duas listagens. Os dois braços completaram as operações sem erros nem cortes pelo logger. O ensaio teve uma sessão por braço, e as contagens cobrem o texto instrumentado.

## Repetição das consultas com o binário final

As 27 chamadas CLI registradas no braço CodeGraph foram repetidas em `26acb4e` e `3e3f136`, com uma série de aquecimento e sete séries pareadas em ordem alternada. As respostas e os códigos de saída coincidiram nas 27 chamadas após normalizar o tempo impresso pela busca. Cada versão produziu 7.721 tokens de saída nessas consultas. A mediana da soma das chamadas caiu de **2.158,312 ms para 813,351 ms (−62,32%)**. A medição desta seção é o tempo dos subprocessos CLI em repetição determinística.

## Medições por melhoria

| Melhoria | Antes | Depois | Escopo |
| --- | ---: | ---: | --- |
| Busca agrupada via CLI | 36.143 tokens | 32.781 tokens | Comando e resposta, 40 consultas em cinco projetos |
| Busca agrupada via MCP | 34.924 tokens | 31.562 tokens | Argumentos e resposta, mesmas 40 consultas |
| Visão geral do projeto | 3.132 tokens | 1.406 tokens | Saída de orientação em cinco projetos, comparada a status, diretórios agregados e entrada limitada |
| Catálogo JSON via CLI | 8.639 tokens | 5.344 tokens | Mesmo catálogo de 31 ferramentas |
| Schema específico via CLI | 8.639 tokens | 113–275 tokens | Catálogo completo anterior e três consultas por nome |
| Atualização por caminho via MCP | 3,176 ms | 0,988 ms | Mediana das medianas de 24 cenários com watcher |
| Atualização por caminho via CLI | 70,579 ms | 68,388 ms | Mediana das medianas de 24 cenários |
| Remoção da espera na descoberta via CLI | 88,357 ms | 79,464 ms | Mediana de três medianas de consulta sobre índices já criados |
| Remoção da espera na descoberta via MCP | 70,671 ms | 54,409 ms | Mediana de três medianas até `initialize`, sem watcher |
| Índice já atualizado via CLI | 68,279 ms | 60,036 ms | Mediana de três medianas de consulta, comparada a `052501f` |
| Índice já atualizado via MCP | 53,569 ms | 48,372 ms | Mediana de três medianas até `initialize`, sem watcher, comparada a `052501f` |

A busca agrupada preservou ranking, arquivo, linhas, assinaturas e saída JSON. A opção economizou tokens em 34 de 40 consultas depois de contar o argumento. A visão geral foi conferida contra o índice e os arquivos, incluindo 32 coordenadas de entrada. Ela mostra amostras de entradas e cobertura indexada. A comparação de orientação usou uma listagem de entradas limitada a 500 tokens no fluxo anterior. O catálogo MCP passou de 5.188 para 5.333 tokens com o parâmetro de agrupamento e a nova ferramenta, e as instruções iniciais passaram de 119 para 125.

As medições de latência usaram binários de desenvolvimento no Windows. A atualização por caminho preservou o texto das 24 consultas e também foi medida em MCP aquecido sem watcher, de 3,010 ms a 0,962 ms. Para descoberta, dez amostras por binário e projeto alternaram a ordem. As medianas CLI foram de 75,415 a 42,007 ms em CodeGraph, 88,357 a 90,304 ms em Nodex e 90,279 a 79,464 ms em chat-on-steroids. As medições do índice já atualizado usaram os mesmos três projetos e preservaram as respostas em ambas as interfaces.

## Cópias do corpus

| Projeto | Commit | Uso |
| --- | --- | --- |
| AI Usage Tracker | `66f4060ed6bfb9822cd15a4360f47e5cdc9ea018` | Ensaio S1–S3 e medições de consulta |
| ParticleEffectForUGUI | `04c1ca72cde82318e11eb1281dc4c3d314a2e68c` | Ensaio S4–S5 e medições de consulta |
| LFG | `efd40913ce9453947da524aeca15ebbccb7fb186` | Ensaio S6–S7 e medições de consulta |
| CodeGraph | `a42e9722114715afb8013072450f2738278b81b9` | Medições de consulta e descoberta |
| Nodex | `22284f34a4f270aadc62de87fb08d78d4f7de2b7` | Medições de consulta e descoberta |
| chat-on-steroids | `7c0bbb64de8bd343f2b57bce3d8f2dce76da5565` | Medições de atualização e descoberta |

Os índices foram criados nas cópias exportadas por `git archive`. Para repetir o ensaio, use as tarefas acima em sessões independentes e registre as leituras conforme o [protocolo do benchmark](../README.md). Para medir consultas, compare `search_universal` com e sem `group_by_file` via `codegraph query` e `tools/call`, retirando o tempo impresso do cabeçalho ao comparar fatos. Use `get_file_outline` com caminho explícito para verificar atualização. Para medir a descoberta, compare `get_project_overview` com `limit=2` via CLI e o tempo até a resposta MCP de `initialize`. Os logs detalhados das fontes ficaram em armazenamento local.
