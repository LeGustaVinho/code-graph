# Benchmark de busca textual e evidência de rotas

A rodada comparou busca textual, trechos de linhas longas e investigação de rotas. As medições de tokens usam `tiktoken` 0.14.0 com `o200k_base`. Os projetos foram exportados por `git archive` e indexados em diretórios temporários.

## Busca textual em arquivos indexados

Os mesmos 16 casos em LFG, Nodex e chat-on-steroids foram executados via CLI e MCP. A otimização de alocações (`7fcb2fb`) preservou as respostas de texto e JSON byte a byte. Cada caso teve uma execução de aquecimento e sete pares de medições nos binários de desenvolvimento.

| Medida | Antes | Depois |
| --- | ---: | ---: |
| MCP, mediana das medianas dos 16 casos | 23,256 ms | 17,162 ms |
| MCP, mediana das medianas das nove buscas globais | 63,863 ms | 43,155 ms |
| CLI, mediana das medianas das nove buscas globais | 91,888 ms | 83,586 ms |
| CLI, mediana das medianas dos 16 casos | 68,485 ms | 69,540 ms |

As 16 consultas MCP ficaram mais rápidas. Na CLI, oito das nove buscas globais ficaram mais rápidas e a mediana dos 16 casos aumentou 1,54%. Em nove buscas globais com binários de release, a soma das medianas caiu de 664,305 para 596,278 ms na CLI e de 374,696 para 293,504 ms no MCP. O JSON ficou idêntico e a maior parte do ganho veio das consultas mais demoradas.

O agrupamento opcional (`fc5ce27`) manteve as 236 coordenadas na mesma ordem em cada transporte, com 141 linhas aproveitando o caminho anterior. A resposta JSON permaneceu idêntica. O texto sem agrupamento já contém o trecho novo para ocorrências fora do início de linhas longas.

| Busca textual | Antes | Depois |
| --- | ---: | ---: |
| Saída CLI com agrupamento | 7.618 tokens | 6.782 tokens |
| Saída MCP com agrupamento | 7.619 tokens | 6.783 tokens |
| Argumentos e saída MCP com agrupamento | 8.089 tokens | 7.349 tokens |
| Ocorrência no fim de linha SQL em LFG, escopo de arquivo | 201 tokens em JSON | 102 tokens em texto |

No último caso, a ocorrência começa depois do caractere 417 de uma linha com 451 caracteres. O texto anterior cortava a ocorrência, que podia ser verificada no JSON. O novo trecho inclui a mesma coordenada e o texto encontrado.

## Evidência de rota

Em dois registros Fastify com wrapper em LFG, o fluxo anterior foi `trace_fullstack_flow` seguido de um `batch_query` de trechos previamente localizados. O modo `route_evidence:true` retornou em uma chamada as evidências equivalentes de registro, passagem do corpo, autenticação no wrapper e chamada de serviço. Foram verificados 14 trechos multilinha diretamente contra as fontes. O benchmark isolado cobriu esses fatos e coordenadas. O ensaio com agentes também considerou as leituras necessárias para completar as respostas.

| Caso | Interface | Tokens antes → depois | Mediana antes → depois |
| --- | --- | ---: | ---: |
| Cancelamento administrativo | CLI | 1.148 → 281 | 72,924 → 21,784 ms |
| Cancelamento administrativo | MCP | 1.147 → 282 | 42,634 → 7,761 ms |
| Publicação de configuração | CLI | 1.017 → 292 | 62,502 → 23,329 ms |
| Publicação de configuração | MCP | 1.016 → 293 | 34,100 → 7,745 ms |

A medição usou binários de release no Windows, com um aquecimento e sete pares. A mediana MCP usa uma sessão persistente com watcher. O webhook registrado em um callback de plugin ficou fora da cobertura do índice neste snapshot e exigiu os mesmos trechos complementares nos dois fluxos. A listagem MCP cresceu de 5.333 para 5.398 tokens com os dois novos parâmetros. As instruções do servidor continuaram com 125 tokens.

Em um recorte de `empires-of-space-2d` com `apps/server` e `packages`, uma rota Fastify direta passou de 549 para 428 tokens na CLI e de 548 para 428 no MCP. A leitura complementar ainda foi necessária para os trechos além do limite de exibição. Esse recorte verificou a saída e as coordenadas.

## Ensaio com duas IAs

Dois agentes com o mesmo modelo receberam as mesmas oito tarefas em sessões independentes, na mesma ordem. Sete tarefas eram novas e a de cancelamento repetiu a pergunta do ensaio anterior. O braço convencional usou buscas textuais, leituras por intervalo e listagem. O braço CodeGraph usou a CLI de release com os recursos da rodada, leu a skill uma vez e fez uma listagem complementar. As contagens incluem as leituras, erros e consultas de verificação registrados. A métrica com argumentos soma a representação dos pedidos no wrapper.

| Tarefa | Convencional | CodeGraph |
| --- | ---: | ---: |
| S0 · Skill | 0 | 796 |
| S1 · Entradas de Electron, preload e renderer | 1.578 | 2.197 |
| S2 · Entradas Tauri, servidor e frontend | 2.187 | 2.220 |
| S3 · Tratamento de duplicata em SQL e validação do lote | 640 | 356 |
| S4 · Ocorrências de comando e campos da ponte | 844 | 189 |
| S5 · Guard de inicialização e teste | 1.946 | 1.526 |
| S6 · Registro, autenticação, corpo e serviço de configuração | 1.444 | 553 |
| S7 · Registro, autenticação, corpo e serviço de cancelamento | 1.531 | 1.644 |
| S8 · Parser, limite e handler de webhook | 332 | 542 |
| **Total de tokens de saída** | **10.502** | **10.023** |
| **Total com argumentos** | **11.532** | **12.486** |
| Operações | 42 | 38 |
| Tempo somado das operações | 1,283 s | 2,499 s |
| Tarefas factualmente corretas | 8 de 8 | 8 de 8 |

A saída caiu **4,56%**, com economia média de 11,74% por tarefa antes da skill. O total com argumentos aumentou **8,27%**. Considerando as sete tarefas novas e a skill, a saída caiu 6,60% e o total com argumentos aumentou 5,85%. Os agentes recuperaram um erro no braço convencional e dois no CodeGraph. O logger não cortou nenhuma resposta. Cada braço teve uma sessão. A medição de tempo cobre as operações instrumentadas.

Os dois mapas de entrada exigiram 18 operações no CodeGraph. No caso de Electron, uma listagem complementar foi necessária para localizar o arquivo de entrada do renderer. No cancelamento, o agente ainda fez quatro consultas agrupadas para resolver aliases e completar a resposta. O caso de webhook exigiu busca textual por falta de registro indexado no callback de plugin.

As 36 chamadas CodeGraph foram repetidas com o binário do commit `3e6677c`. Após normalizar os tempos impressos pela busca, 34 respostas ficaram idênticas. As outras duas mudaram apenas o cabeçalho, que explica a base das coordenadas relativas. O corpo, o stderr e os códigos de saída permaneceram iguais. O total observado na tabela continua sendo o das sessões originais.

## Projetos

| Projeto | Commit |
| --- | --- |
| LFG | `efd40913ce9453947da524aeca15ebbccb7fb186` |
| Nodex | `22284f34a4f270aadc62de87fb08d78d4f7de2b7` |
| chat-on-steroids | `7c0bbb64de8bd343f2b57bce3d8f2dce76da5565` |
| empires-of-space-2d (recorte do servidor) | `85ed32121771667effa3e58d3d0435f0565513bf` |

As capturas detalhadas ficaram em armazenamento local. O [protocolo do benchmark](../README.md) descreve o registro de leituras e a contagem por sessão.
