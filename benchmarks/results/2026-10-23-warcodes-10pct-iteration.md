# CodeGraph: seleção de vários membros e economia de tokens

**Data:** 2026-10-23. **Projeto de validação:** Warcodes Mobile (Unity/C#), snapshot `2a1406b3f3b0aa27a60c1857d153fb346dd2f423`.

**Resultado:** economia ponderada de **37,9%**, incluindo a skill; média aritmética entre cenários de **19,6%**. Ambas superam a meta de 10%. O candidato foi executado por um novo subagente independente. A baseline convencional de 9.246 tokens é a execução separada e previamente validada descrita em `2026-10-23-warcodes-mobileapp-read-benchmark.md`, no mesmo snapshot e com as mesmas sete perguntas.

## Comparativo (o200k_base)

| Cenário | Convencional | CodeGraph | Economia |
| --- | ---: | ---: | ---: |
| S1: assinaturas de quatro métodos | 1.011 | 136 | +86,5% |
| S2: retry assíncrono e cleanup | 1.522 | 2.229 | −46,5% |
| S3: caller e transição anterior | 946 | 420 | +55,6% |
| S4: fallback de prefab e exceções | 884 | 1.194 | −35,1% |
| S5: interface e implementadores diretos | 3.418 | 420 | +87,7% |
| S6: ocorrências textuais, incluindo comentário | 1.307 | 419 | +67,9% |
| S7: referências diretas de testes | 158 | 283 | −79,1% |
| **Operações de leitura** | **9.246** | **5.101** | **+44,8%** |
| Instruções da skill | 0 | 640 | — |
| **Total comparado** | **9.246** | **5.741** | **+37,9%** |

A skill anterior tinha 901 tokens; a nova tem 640, preservando cautelas sobre índices incompletos, métodos privados, overloads, chamadas dinâmicas e evidência parcial. O total de operações foi 19 para cada braço. O tempo somado registrado do candidato foi 6,419 s; esses tempos não representam o tempo completo de resposta de um agente.

## Mudanças reaproveitáveis

- `get_file_outline(file_path,names:[...],include_signatures:true)` retorna vários métodos com assinaturas, linha e ancestral no mesmo resultado. O filtro `filter` por substring permanece disponível e é mutuamente exclusivo com `names`.
- `get_flow_evidence(symbol_id,members:[...])` seleciona vários membros exatos de um tipo e reduz chamadas individuais, mantendo alertas sobre trechos omitidos. `filter` por substring permanece disponível.
- A skill passa a recomendar consultas diretas para símbolos conhecidos, uso parcimonioso de busca fuzzy e seleção conjunta quando várias declarações precisam ser examinadas. Não há nomes de projetos, símbolos ou linguagens especiais nas regras.
- Os testes adicionais cobrem C#, TypeScript, Python e Rust. O smoke geral de linguagens suportadas também passou.

## Qualidade e falhas

O agente candidato apresentou evidência correta e equivalente para S1–S7, incluindo progressão Fibonacci e cancelamento do retry; transição antes de `LoadGameModeScene`; fallback e captura de exceções; dez propriedades de `IBattleResultsProxy` e dois implementadores diretos; dez referências a `PanelDataHandler.BindPanelData` (uma comentada); e ausência de referências C# diretas a `BattleModeSceneLoader` em testes. Para S7, a consulta CodeGraph revelou que `Tests` não tinha arquivos indexados, seguida por buscas convencionais **medidas** em `Assets` e `Tests`. Dois exit codes 1 em S7 são esperados: escopo não indexado e `rg` sem arquivos C# sob `Tests`.

Uma tentativa exploratória de repetir a baseline convencional gerou dois dumps não filtrados de `rg --files` (388.462 tokens) antes da filtragem feita pelo shell. O log bruto foi preservado em `target/tmp/read_meter_plain_warcodes_fixbase.jsonl`, mas não usado para fundamentar economia. O comparativo usa a baseline independente já validada e documentada, sem remover operações da candidata.

## Reprodução e limites

- Baseline: `target/tmp/read_meter_plain_warcodes.jsonl`.
- Candidata: `target/tmp/read_meter_cg_warcodes_multi.jsonl`.
- Agregado: `benchmarks/results/2026-10-23-warcodes-10pct-iteration.json`.

O tokenizador é `tiktoken/o200k_base`; o custo contabilizado é o texto de respostas das ferramentas, com custo bruto da skill, incluindo falhas e fallbacks. Não inclui tokens de raciocínio oculto, argumentos de chamadas ou faturamento de APIs. É uma execução por braço num projeto; o comportamento de economia geral deve ser verificado em outras bases e tarefas. Em particular, S2, S4 e S7 ainda demandam melhorias. Nenhum arquivo de código do projeto de teste foi editado.
