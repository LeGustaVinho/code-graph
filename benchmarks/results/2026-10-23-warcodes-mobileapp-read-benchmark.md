# Benchmark de tokens de leitura — Warcodes Mobile (Unity/C#)

**Data:** 2026-10-23. **Projeto:** `D:/Entidades/EmergentsWorlds/warcodes-mobileapp`, Git HEAD `2a1406b3f3b0aa27a60c1857d153fb346dd2f423`. **CodeGraph:** Git HEAD `a42e9722114715afb8013072450f2738278b81b9`; CLI `0.1.0`, release SHA-256 `CEBB3FA8A087AA205541B71009E34FE09D62C98A7A4B5378EE1E8BF09E90C5FD`. Ambas as working trees possuíam alterações anteriores. Nenhuma dessas alterações foi sobrescrita.

## Protocolo

Dois subagentes, `worker-1` (skill CodeGraph + CLI) e `worker-2` (buscas convencionais `rg` e leituras de faixas), receberam as mesmas sete perguntas, sem compartilhar contexto ou resultados. O agente principal fez revisão de qualidade à parte. O projeto fonte permaneceu sem alterações de código.

Todas as leituras que retornaram texto foram executadas com o medidor temporário `target/tmp/read_meter.py`, separado por `--run cg_warcodes` e `--run plain_warcodes`. O medidor grava operação, status, duração, contagem e hash SHA-256 da resposta em JSONL. A contagem usa `tiktoken/o200k_base` sobre `stdout+stderr` de comandos ou faixas numeradas de arquivos. Erros, consultas adicionais e resultados sem correspondência são contabilizados; o custo do texto da skill (830 tokens) é mostrado em separado. Não são medidos tokens de raciocínio dos agentes, tokens da chamada da ferramenta, faturamento real de modelos nem toda a saída de setup feita fora do medidor. Isso mede **tokens de texto de operações de leitura**, não consumo total da sessão.

## Resultado

| Cenário | Tradicional | CodeGraph | Economia |
|---|---:|---:|---:|
| S1. EventHubController: assinaturas e linhas de quatro métodos | 1.011 | 1.331 | −31,7% |
| S2. RetryEmptyEventsAsync: disparo, encerramento, backoff e exceções | 1.522 | 1.851 | −21,6% |
| S3. Caller e transição anterior a LoadGameModeScene | 946 | 1.119 | −18,3% |
| S4. Fallback de prefab e exceções do BattleModeSceneLoader | 884 | 1.621 | −83,4% |
| S5. IBattleResultsProxy: propriedades e implementadores diretos | 3.418 | 1.599 | +53,2% |
| S6. PanelDataHandler.BindPanelData: referências em Assets/Game | 1.307 | 1.022 | +21,8% |
| S7. Referências diretas de testes a BattleModeSceneLoader | 158 | 256 | −62,0% |
| **Total das consultas** | **9.246** | **8.799** | **+4,8%** |
| Skill CodeGraph (texto bruto) | 0 | 830 | — |
| **Total com a skill** | **9.246** | **9.629** | **−4,1%** |

Economia ponderada (consultas apenas): **+4,835%**. Economia ponderada (incluindo a skill): **−4,142%**. Média aritmética de economias dos sete cenários (sem ratear a skill): **−20,275%**. O limiar de 20% estabelecido no protocolo do repositório não foi atingido.

Foram **19 operações** convencionais e **32 consultas** CodeGraph. A soma de tempos registrada pelos medidores foi **7,40 s** contra **54,98 s**, respectivamente, incluindo a consulta inicial CodeGraph de status (~14,2 s) e a primeira busca de símbolo (~30,3 s). Esses tempos são somas de execuções, não tempos completos de agente; não incluem toda a configuração ou esperas externas.

## Equivalência e inconsistências

- **S1–S4:** revisadas em código: `EventHubController` contém métodos nas linhas 41, 73, 208 e 245. O retry inicia para dados de evento carregados com lista vazia, utiliza atrasos 1, 1, 2, 3, 5... s com limite `int.MaxValue/1000`, cancela ao receber eventos/descartar o controller e trata cancelamento e exceções. `LoadGameModeScene` parte de `Update` após estado `Initialized`; a substituição de prefab falha para o padrão quando faltam dados, o componente é inválido ou a instanciação/initialização falha.
- **S5:** a interface declara dez propriedades e tem **dois implementadores diretos** em `Assets/Game`: `BattleResultsGenericProxy` e `BattleResultsProxy` (KingOfTheHill), ambos na linha 11 de seus arquivos. Outras classes podem herdar de implementadores; isso não as torna implementações *diretas*.
- **S6:** dez ocorrências textuais: nove chamadas em código executável e um trecho comentado em `MockCollectibleItemLoader.cs:63`. `search_universal` também apresentou `IPanelDataHandler.BindPanelData` como candidato lexical parecido; a revisão com evidência de chamadas descartou-o. A correspondência semântica indiscriminada teria dado um falso positivo.
- **S1:** `get_file_skeleton` omitiu `Start` e `OnDestroy` por serem privados; `get_file_outline` posterior mostrou ambos. É uma diferença de cobertura entre visões selecionadas, não prova de ausência dos símbolos. As consultas extras contam no benchmark.
- **S7:** a busca CodeGraph em `Tests` falhou com `no indexed files under path Tests` (exit 1), que **não significa zero referências**. `get_tests_for_symbol` indicou zero testes ligados, mas não inspeciona de forma conclusiva um diretório fora do índice. A revisão independente convencional encontrou zero referências C# em `Tests` e referências apenas de aplicação em `Assets`. A resposta do braço CodeGraph tem uma ressalva de cobertura para esse escopo, enquanto não houver fallback registrado.
- **Instrumentação:** houve um `UnicodeEncodeError` (console CP1252) ao emitir texto de uma operação S2; o medidor havia registrado o resultado e o status 0, mas a emissão ao agente falhou. A execução continuou após usar saída UTF-8. Essa entrada foi **mantida na contagem**, podendo superestimar o texto efetivamente visto pelo agente CodeGraph.
- **Setup:** uma chamada de setup `status --root .` retornou exit 2, e um pipeline `tools --json | Select-Object -First 26` retornou exit −1. Executados fora do medidor, são registrados qualitativamente, sem cobrança de tokens. O `status .` medido em S1 retornou nove arquivos desatualizados no índice, zero arquivos com erro e 1.447 arquivos indexados; a atualização automática subsequente deve ser considerada ao comparar tempos.
- **Statuses instrumentados:** o braço convencional registrou um `rg` exit 1 esperado pela ausência de correspondências em `Tests`; CodeGraph registrou um exit 1 que indica escopo sem arquivos indexados. Não ocorreram bloqueios de banco SQLite nesta execução.

A equivalência foi conferida nos fatos de S1–S6. Em S7, as conclusões coincidem com a revisão independente, mas a evidência interna da ferramenta semântica é insuficiente no escopo não indexado. Portanto, os **−4,1%** representam o comparativo observado de saída de leitura, e não uma garantia de qualidade integralmente equivalente. Uma única execução por braço e apenas um repositório Unity/C# não permitem inferir economia para outros projetos ou linguagens.

## Logs e reprodução

O material de medição de origem é mantido localmente em `/codegraph/target/tmp/read_meter_cg_warcodes.jsonl` e `/codegraph/target/tmp/read_meter_plain_warcodes.jsonl`. Contêm metadados das operações e hashes das saídas; não armazenam o corpo integral do código proprietário. O auxiliar `target/tmp/read_meter.py` fica em diretório ignorado pelo Git.

O executável CodeGraph é `D:/Entidades/Gustavo/Projects/CodeGraph/target/release/codegraph.exe`. Os resultados medidos correspondem a respostas `compact` da skill de 2026-10-21, com aumento de detalhe quando necessário. Um novo teste com outras tarefas, linguagens e amostras deve usar os mesmos critérios de correção e de contagem antes de fazer afirmações gerais de economia.
