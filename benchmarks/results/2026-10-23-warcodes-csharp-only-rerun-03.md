# Repetição isolada do benchmark CodeGraph — Warcodes Mobile, C# somente

**Data:** 23/10/2026. **Projeto:** `D:/Entidades/EmergentsWorlds/warcodes-mobileapp` (HEAD `00b453eb43c0dee6dff8c4194b4464c65b01c47e`). **CodeGraph:** HEAD `a42e9722114715afb8013072450f2738278b81b9`, binário SHA-256 `076010B0A51A61F97E1150B49061BBC5A76A0E545991948B2A185B7F16E91AF2`. **Skill instalada:** SHA-256 `e9b94edec435c3f8f3a4052ff67d09ab829d4bd5275220e787bbbab78eaddde8`.

## Protocolo

Foram iniciados **dois subagentes novos e independentes**, com sete perguntas idênticas e sem troca de contexto ou resultados. O braço CodeGraph usou a skill instalada atual e CLI via `benchmarks/query_logger.py`; o braço convencional usou buscas `rg --glob '*.cs'` e leituras de faixas C#, também via medidor. Todos os comandos de leitura, retries, fallbacks e respostas de erro foram computados. Os subagentes não consultaram logs/resultados anteriores. A revisão da exatidão foi externa à medição, pelo agente principal.

**Escopo exclusivo:** arquivos `.cs`. Nenhuma leitura de `.prefab`, `.unity`, `.json`, `.asset` ou demais formatos de configuração/serialização. As buscas negativas e a ausência de arquivos indexados não foram confundidas. A auditoria de `operations.jsonl` não encontrou leituras não C# nem buscas convencionais sem `--glob '*.cs'`.

A métrica usa `tiktoken/o200k_base` sobre `stdout+stderr` registrado, incluindo buscas sem resultados e erros, com o texto da skill computado **uma vez** (`S0.log`, 682 tokens). Não mede tokens de raciocínio, faturamento, mensagens dos agentes ou custo completo de contexto e indexação.

## Resultado

| Cenário | Convencional | CodeGraph | Economia |
|---|---:|---:|---:|
| S1. Métodos e ciclo de vida de `UserProfilePanel` | 1.750 | 854 | +51,2% |
| S2. Permissão e fluxo de `BarcodeScannerWrapper` | 6.620 | 5.131 | +22,5% |
| S3. `WorldNavigationHelper`, rotas e chamadas C# | 772 | 1.223 | −58,4% |
| S4. Cache e falhas em `DisplayScriptResolver` | 1.244 | 2.040 | −64,0% |
| S5. `IDisplayScriptResolver` e implementações diretas | 383 | 515 | −34,5% |
| S6. Inicialização `DisplayScripts` em `FirebaseManager` | 3.717 | 2.813 | +24,3% |
| S7. Referências textuais em testes C# | 160 | 478 | −198,7% |
| **Total das leituras** | **14.646** | **13.054** | **+10,9%** |
| Texto da skill | **0** | **682** | — |
| **Total com skill** | **14.646** | **13.736** | **+6,2%** |

**Economia ponderada com a skill:** +6,2133% (**910 tokens**). **Média aritmética por cenário, sem ratear a skill:** −36,8012%. Assim, o benchmark **não atingiu** a meta de economia de 20% nos dois critérios. O consumo foi inferior no CodeGraph em S1, S2 e S6, mas maior em S3, S4, S5 e S7.

Foram **23 operações convencionais**, das quais 22 concluídas e uma busca válida sem correspondências; e **38 operações CodeGraph**, das quais 36 concluídas, uma busca válida sem correspondências e uma falha. Dessas 38, **três foram fallbacks convencionais** contabilizados no braço CodeGraph. Tempos de execução somados pelo medidor: **236 ms** convencional e **5.115 ms** CodeGraph. São tempos dos comandos, não duração dos agentes.

## Revisão de qualidade

Os dois braços responderam aos sete cenários com fatos equivalentes. Em S1, identificaram os três métodos públicos e os métodos Unity `Start`/`OnDestroy` em `UserProfilePanel.cs`. Em S2, cobriram os ramos de permissão Android, iOS/WebGL e demais plataformas, inicialização, reentrada e pausa da câmera, inclusive `StopScanning`.

Em S3, os dois distinguiram `GotoArea(string)`, `GotoArea(AreaData)`, `GotoAreaByRef`, `GotoAreaLocation` e `NavigateToAreaLocation`. **A busca apenas em `.cs` encontrou a declaração `WorldNavigationHelper.cs:69`, sem outros callsites C# diretos de `GotoAreaLocation` em `Assets/Game`.** Nenhum prefab entrou no teste.

Em S4, ambos cobriram chave de cache, reutilização de `Task<GameObject>`, validação de dados e remoção da entrada após falha retornando nulo, além do risco de task excepcional permanecer em cache. Em S5, apontaram `DisplayScriptResolver.cs:14` como única implementação direta de `IDisplayScriptResolver`. Em S6, localizaram a criação do resolver com dados mockados no Editor (`FirebaseManager.cs:378`) e o fallback/live (`:391`). Em S7, não encontraram arquivos de teste C# com o texto `DisplayScriptResolver` nos escopos pedidos.

## Erros e cobertura

A consulta semântica `search_source(query=DisplayScriptResolver,path=Tests)` retornou exit 1, informando que o diretório `Tests` **não tem arquivos fonte indexados**. Isso não comprova ausência de referências. O agente CodeGraph complementou com busca `rg` convencional filtrada em `.cs`, registrada no mesmo braço, que terminou sem correspondências. O braço convencional também obteve `rg` exit 1 válido para a busca em `Tests`; não houve falhas inesperadas nele.

O CodeGraph reportou 598 arquivos indexados em `Assets/Game`, com nenhum erro de leitura ou arquivo obsoleto excluído naquele escopo e 11 leituras UTF-8 com perdas. Não foram identificadas divergências factuais relevantes nas sete respostas.

## Comparabilidade e reprodução

Esta repetição usou o **mesmo HEAD do projeto e o mesmo conteúdo de skill e binário** da bateria C# corrigida anterior (`2026-10-23-warcodes-csharp-only-corrected.md`), que havia observado **−21,9%** de economia ponderada com a skill. Agora foram **+6,2%**. Essa variação entre execuções isoladas evidencia a sensibilidade do resultado à estratégia de consulta de cada agente, mesmo com as mesmas perguntas e versão da ferramenta. Uma execução por braço não fornece estimativa estatisticamente estável.

Os logs completos incluem trechos de código proprietário e estão apenas localmente, fora do Git, em `target/tmp/bench_20261023_cs_only_03/baseline` e `target/tmp/bench_20261023_cs_only_03/cg`. O comparador salvou `target/tmp/bench_20261023_cs_only_03/comparison.json` e pode ser reexecutado com:

```powershell
python benchmarks/compare_reading.py --baseline target/tmp/bench_20261023_cs_only_03/baseline --candidate target/tmp/bench_20261023_cs_only_03/cg --output target/tmp/bench_20261023_cs_only_03/comparison.json
```

Nenhum arquivo de código do aplicativo foi alterado pelos agentes. Alterações preexistentes nas árvores de trabalho foram preservadas.
