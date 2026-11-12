# Benchmark de localização de arquivos, trechos e plugins Fastify

As medições de tokens usam `tiktoken` 0.14.0 com `o200k_base`. As consultas compararam snapshots equivalentes, com binários de release no Windows, um aquecimento e sete pares por cenário. A latência CLI inclui o processo. A latência MCP mede a chamada em sessão persistente. Os índices foram criados em cópias temporárias dos projetos.

## Localização de arquivos e trechos

Em nove consultas em quatro projetos, `find_files` (`cba1c6f`) devolveu as mesmas 36 localizações que a busca por símbolos de módulo, conferidas no índice e nos arquivos. A bateria incluiu ausência no índice.

| Interface | Saída antes → depois | Argumentos e saída antes → depois | Soma das medianas antes → depois |
| --- | ---: | ---: | ---: |
| CLI | 1.761 → 590 tokens | 2.289 → 1.107 tokens | 267,951 → 215,790 ms |
| MCP | 1.754 → 582 tokens | 2.028 → 845 tokens | 50,800 → 3,343 ms |

A listagem ampla com `rg --files`, filtrada por extensão, custou 22.417 tokens. Uma busca `rg -g` já direcionada por nome e extensão devolveu os mesmos caminhos com 442 tokens. A saída de `find_files` também informa a cobertura do índice.

A correção de busca literal (`a5ed4b4`) resolveu a ambiguidade entre `open` e `Command.Open` ao consultar o arquivo de entrada do servidor Nodex. O mesmo corpo passou de duas operações, incluindo o erro e a recuperação por ID, para uma. A saída caiu de 382 para 214 tokens em ambas as interfaces. Argumentos e saída passaram de 461 para 253 tokens na CLI e de 455 para 250 no MCP.

O modo `get_symbol_body(file_path,start_line,end_line)` (`469c980`) foi verificado em cinco recortes de cinco snapshots. As 57 linhas mantiveram coordenadas, conteúdo e indentação. O fluxo anterior descobria o ID do módulo e pedia o intervalo em uma segunda chamada.

| Interface | Saída antes → depois | Argumentos e saída antes → depois | Soma das medianas antes → depois |
| --- | ---: | ---: | ---: |
| CLI | 1.368 → 838 tokens | 2.051 → 1.166 tokens | 288,035 → 119,045 ms |
| MCP | 1.369 → 839 tokens | 1.770 → 1.021 tokens | 36,657 → 2,681 ms |

O fluxo anterior com ID já conhecido consumia 964 tokens de saída na CLI. A leitura convencional direta com numeração custou 696 tokens de saída. O trecho de Nodex cruza as declarações de `main` e `open`, o que o modo restrito a um único símbolo rejeita.

## Plugin Fastify

A extração de callbacks inline (`2eb55c6`) passou a indexar o webhook de LFG na linha 985. A fixture de prefixos passou de uma para quatro rotas esperadas no CLI e no MCP, com controles de escopo e de outro framework. Prefixos são vinculados à instância. As rotas de um receiver externo mantêm seu prefixo quando chamadas de um callback interno.

Para explicar o registro, o parser e o handler do webhook, o fluxo anterior dependia de dois trechos. Após o ajuste do extrator, a consulta de rota encontrou o handler e restou um trecho de parser. Na CLI, a saída caiu de 473 para 299 tokens e o total com argumentos caiu de 609 para 399. No MCP, a saída caiu de 472 para 299 e o total de 601 para 392. As medianas foram de 43,397 a 43,011 ms na CLI e de 40,458 a 16,018 ms no MCP.

Uma cópia de índice `+x12` já existente foi atualizada automaticamente para `+x13` em cada interface. As amostras de inicialização foram de 747 ms na primeira consulta CLI e de 670 ms até `initialize` do MCP. A rota já estava disponível na primeira consulta em ambas.

A evidência de parser (`259678c`) reuniu o registro, o parser e o handler em uma consulta. Os três trechos foram conferidos nas coordenadas originais. Em relação ao fluxo anterior a esta rodada, o resultado foi o seguinte.

| Interface | Saída antes → depois | Argumentos e saída antes → depois | Mediana antes → depois |
| --- | ---: | ---: | ---: |
| CLI | 473 → 194 tokens | 609 → 235 tokens | 43,484 → 20,861 ms |
| MCP | 472 → 194 tokens | 601 → 231 tokens | 18,805 → 8,164 ms |

O parser aparece como trecho anterior do mesmo receiver e bloco. Quando dois registros diretos do mesmo verbo compartilham a linha, a resposta mantém a linha indexada sem associar o parser a um deles. Registros prefixados cujo caminho local difere do caminho indexado também podem aparecer apenas como localização. Arquivos de configuração continuam com a leitura de chaves via `get_config_matrix` (`c975192`).

## Ensaio com duas IAs

Dois agentes com o mesmo modelo receberam sete tarefas na mesma ordem, em sessões independentes. Três tarefas repetiram perguntas do ensaio anterior e quatro eram novas. O agente convencional fez buscas textuais, listagens direcionadas e leituras numeradas. O outro usou a CLI de release e leu a skill uma vez. A contagem de argumentos usa a representação dos pedidos registrada pelo wrapper.

| Tarefa | Convencional | CodeGraph |
| --- | ---: | ---: |
| S0 · Skill | 0 | 849 |
| S1 · Mapa Electron, preload e renderer | 941 | 607 |
| S2 · Entradas Tauri, servidor e frontend | 1.797 | 1.362 |
| S3 · Parser, limite e handler de webhook | 310 | 194 |
| S4 · Janela, preload e renderer de overlay | 275 | 276 |
| S5 · Arquivos da API e interface de billing | 56 | 179 |
| S6 · Componente Unity e vínculo com Inspector | 67 | 174 |
| S7 · Autenticação e tratamento de eventos de conta | 1.234 | 770 |
| **Total de tokens de saída** | **4.680** | **4.411** |
| **Total com argumentos** | **5.238** | **5.491** |
| Operações | 24 | 19 |
| Tempo somado das operações | 0,500 s | 1,177 s |
| Tarefas factualmente corretas | 7 de 7 | 7 de 7 |

Com a skill, o CodeGraph economizou **5,75% na saída**. Argumentos e saída somados ficaram **4,83% acima** do braço convencional. A média aritmética do consumo de saída por tarefa aumentou 35%, com a skill contabilizada separadamente e maior peso proporcional nas tarefas curtas S5 e S6. As três tarefas repetidas custaram 3.048 tokens de saída no braço convencional e 2.163 com CodeGraph, antes do custo da skill. No ensaio anterior, o braço CodeGraph gastou 4.959 tokens de saída nas mesmas perguntas, em uma sessão com outro conjunto de tarefas.

O agente convencional fez 13 buscas, sete leituras e quatro listagens. O braço CodeGraph fez 18 consultas e uma leitura da skill, sem fallback. Os dois agentes concluíram a sessão sem erros nem cortes pelo logger. A visão geral em S2 incluiu metadados e duas coordenadas C# fora do recorte. As leituras de código posteriores e as citações da resposta permaneceram nos tipos de arquivo solicitados. Cada braço teve uma sessão. Os tempos cobrem as operações instrumentadas.

O catálogo MCP cresceu de 5.398 para 5.589 tokens e de 31 para 32 ferramentas. As instruções iniciais do servidor mantiveram 125 tokens. O ensaio de IAs usou CLI, com a skill de 849 tokens contabilizada na tabela.

As 18 chamadas CLI do ensaio foram repetidas no binário publicado (`259678c`). Os textos, stderr e códigos de saída coincidiram após normalizar os tempos impressos pela busca. Cada versão produziu 3.562 tokens nessas chamadas, antes da skill.

## Projetos

| Projeto | Commit | Recorte |
| --- | --- | --- |
| chat-on-steroids | `7c0bbb64de8bd343f2b57bce3d8f2dce76da5565` | Snapshot completo |
| Nodex | `22284f34a4f270aadc62de87fb08d78d4f7de2b7` | Snapshot completo |
| LFG | `efd40913ce9453947da524aeca15ebbccb7fb186` | Snapshot completo |
| UIEffect | `70937d2ce39b61c29c4feb4d13642c65bb553d6c` | `Packages/src` |
| empires-of-space-2d | `85ed32121771667effa3e58d3d0435f0565513bf` | `apps/server` e `packages` |

As capturas detalhadas ficaram em armazenamento local. O [protocolo de benchmark](../README.md) descreve o registro de leituras e a contagem por sessão.
