# Benchmark de busca multi-termo e custo de contexto

As contagens usam `tiktoken` 0.14.0 com `o200k_base`. O custo com argumentos soma a representação dos pedidos registrada pelo wrapper. As medições de consulta usaram binários de release no Windows, sete pares e um aquecimento. A latência CLI inclui o processo. A latência MCP mede chamadas em sessão persistente.

## Skill e catálogo

A skill caiu de 849 para 572 tokens em `90ec25f`, com as regras de escopo, cobertura e confiança preservadas. Com a explicação da busca multi-termo, a versão fixada no ensaio ficou com 582 tokens, uma redução de 31,45%. O catálogo MCP passou de 5.589 para 5.628 tokens com o novo parâmetro. Na versão publicada com o recorte de visão geral, a skill ficou com 591 tokens e o catálogo com 5.655. O servidor manteve 32 ferramentas e 125 tokens de instruções iniciais.

## Busca com vários termos

Em oito workloads de quatro projetos, `search_source(queries:[...])` (`948563e`) pesquisou 31 termos. Em cada transporte, as 106 linhas exibidas coincidiram em arquivo, ordem, linha e trecho. A cobertura também coincidiu, incluindo 874 ocorrências contadas antes do limite, seis termos sem resultados e uma linha SQL longa. A consulta lê e decodifica cada arquivo uma vez no escopo compartilhado.

| Interface e medida | Consultas individuais | Batch de consultas individuais | Busca multi-termo |
| --- | ---: | ---: | ---: |
| CLI, saída | 4.188 tokens | 4.405 tokens | 3.334 tokens |
| CLI, argumentos e saída | 6.274 tokens | 5.969 tokens | 4.008 tokens |
| CLI, soma das medianas | 1.125,883 ms | 550,587 ms | 345,686 ms |
| MCP, saída | 4.189 tokens | 4.405 tokens | 3.334 tokens |
| MCP, argumentos e saída | 5.401 tokens | 5.746 tokens | 3.777 tokens |
| MCP, soma das medianas | 351,315 ms | 345,120 ms | 150,732 ms |

As 31 chamadas individuais passaram a oito chamadas. O batch também usa oito chamadas, com uma passagem pelo escopo por termo. Nos dois workloads globais de chat-on-steroids e Nodex, a soma das medianas caiu de 388,762 para 199,653 ms na CLI e de 322,926 para 138,331 ms no MCP em relação ao batch.

## Contexto por caminho e atualização do índice

`get_project_overview(path)` (`c859761`) foi validado em sete recortes de quatro projetos. A comparação com o índice confirmou 296 arquivos de código, 21 áreas e 22 coordenadas de pontos de entrada. O recorte de desktop em Nodex retornou cinco arquivos Rust, duas áreas e cinco pontos de entrada. O recorte da API de LFG contou 30 arquivos TypeScript e 149 pontos de entrada, com oito amostras. A visão global continuou idêntica, inclusive com `path='.'`.

As referências `arquivo:símbolo` passaram a seguir imports comprovados (`cb9db8c`). Em JavaScript e TypeScript, o novo caminho verifica exportação e a resolução relativa direta do módulo. O resolver recusa o caso reproduzido de dois módulos que reexportam o mesmo nome por `export *`. A consulta com alvo já indexado também atualiza o novo arquivo quando o importador e o alvo mudam juntos.

Em três aliases reais de LFG, Nodex e chat-on-steroids, a busca pelo import seguida da leitura do alvo passou de duas chamadas para uma, preservando o corpo de cada símbolo. Na CLI, a saída passou de 354 para 105 tokens e o total com argumentos caiu de 720 para 291. No MCP, a saída passou de 355 para 105 e o total com argumentos caiu de 553 para 207. Com o índice por arquivo aplicado à consulta em `1cd5712`, a soma das medianas caiu de 139,201 para 71,418 ms na CLI e de 2,846 para 2,134 ms no MCP. Em uma medição isolada com as mesmas três consultas de alias e saída idêntica, o ajuste do índice reduziu a soma das medianas de 63,775 para 2,434 ms no MCP.

A atualização de `batch_query` reúne referências válidas de operações diretas e atualiza o índice antes de executar o lote. Em uma fixture MCP com `--no-watch`, o lote anterior devolvia `old.ts:1` após o import mudar de alvo. O lote atualizado devolveu `new.ts:3` e o novo corpo já na primeira chamada. Leituras de intervalo continuam lendo a fonte ao vivo. Lotes malformados são recusados antes da atualização direcionada.

Em cópias de LFG e Nodex com dois arquivos editados em cada projeto, o primeiro lote MCP devolveu zero dos quatro símbolos novos antes de `7b4722a` e os quatro após o ajuste. O fluxo que tentava o lote e depois recuperava a resposta em duas consultas custou 578 tokens com argumentos. O lote atualizado custou 322. Duas consultas diretas corretas desde o início custaram 252. Os processos CLI produziram as mesmas respostas, pois a abertura já atualiza o índice.

## Ensaio com duas IAs

Dois agentes com o mesmo modelo responderam a sete tarefas em sessões independentes. Três perguntas repetiram cenários anteriores e quatro eram novas. A leitura convencional usou buscas direcionadas e intervalos de arquivo. O braço CodeGraph usou o binário de `948563e` e carregou a skill curta uma vez. Os 3.352 arquivos rastreados foram comparados por SHA-256 entre as cópias, com os índices separados.

| Projeto | Revisão e escopo |
| --- | --- |
| chat-on-steroids | `7c0bbb64de8bd343f2b57bce3d8f2dce76da5565`, repositório completo |
| Nodex | `22284f34a4f270aadc62de87fb08d78d4f7de2b7`, repositório completo |
| LFG | `efd40913ce9453947da524aeca15ebbccb7fb186`, repositório completo |
| UIEffect | `70937d2ce39b61c29c4feb4d13642c65bb553d6c`, `Packages/src` |

| Tarefa | Convencional | CodeGraph |
| --- | ---: | ---: |
| S0 · Skill | 0 | 582 |
| S1 · Entradas Electron, preload e renderer | 91 | 169 |
| S2 · Entradas Tauri, servidor e frontend | 225 | 369 |
| S3 · Parser, limite e handler de webhook | 212 | 194 |
| S4 · APIs e canais de dois preloads | 244 | 521 |
| S5 · Segunda instância e encerramento do desktop | 1.287 | 912 |
| S6 · Filtros de duas consultas SQL | 314 | 429 |
| S7 · Inicialização de proxies em Unity | 849 | 783 |
| **Total de tokens de saída** | **3.222** | **3.959** |
| **Total com argumentos** | **3.787** | **4.811** |
| Operações | 18 | 15 |
| Tempo somado das operações | 0,130 s | 0,535 s |
| Tarefas factualmente corretas | 7 de 7 | 7 de 7 |

Com a skill, a saída cresceu **22,87%** e o total com argumentos cresceu **27,04%**. Cada braço teve uma sessão, sem erros nem cortes do logger. O agente convencional fez 11 buscas e sete leituras. O braço CodeGraph fez 14 consultas CLI, seis delas multi-termo, mais uma leitura da skill. As consultas convencionais para os dois mapas de entrada foram mais direcionadas nesta sessão. Os tempos cobrem as operações instrumentadas.

Mantidas as mesmas operações, trocar apenas a contagem da skill de 582 para 849 levaria o total com argumentos de 4.811 para 5.078 tokens. Essa conta mede os 267 tokens de configuração poupados. A estratégia do agente foi observada com a skill curta.

As seis consultas multi-termo escolhidas no ensaio foram repetidas como buscas individuais e como batches. Mantiveram as mesmas evidências e cobertura. A saída foi de 1.925 tokens em buscas individuais e 2.079 em batch, ante 1.462 no multi-termo. Com argumentos CLI, foram 3.401 e 3.166 tokens, ante 1.953. A diferença nesses seis workflows mede o ganho do recurso mesmo quando a sessão completa favorece a leitura convencional.

O binário publicado em `1cd5712` repetiu as 14 chamadas CLI do ensaio com conteúdo, stderr e códigos de saída idênticos após normalizar somente o tempo informado. A saída dessas chamadas somou 3.377 tokens em ambos os binários. Em cinco pares de execuções, a soma das medianas passou de 443,288 para 435,847 ms.

As capturas detalhadas ficaram em armazenamento local. O [protocolo do benchmark](../README.md) descreve o registro das leituras.
