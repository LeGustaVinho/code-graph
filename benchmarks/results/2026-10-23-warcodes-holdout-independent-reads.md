# Benchmark independente de leituras — Warcodes Mobile (Unity/C#)

**NOTA DE ESCOPO — resultado histórico substituído:** este benchmark incluiu indevidamente leituras de prefabs Unity em S3 e leu o manifesto JSON em S6. Ele **não atende ao escopo exclusivo `.cs`** definido pelo usuário. Use a execução corrigida em `2026-10-23-warcodes-csharp-only-corrected.md` para comparar operações limitadas a C#; os resultados desta versão foram preservados apenas como histórico.

**Data:** 23/10/2026. **Fonte:** `D:/Entidades/EmergentsWorlds/warcodes-mobileapp`, Git `5080c8844e69c336f4671f652b274aaa0fa2cc2b`. **CodeGraph:** Git `a42e9722114715afb8013072450f2738278b81b9`, executável SHA-256 `076010B0A51A61F97E1150B49061BBC5A76A0E545991948B2A185B7F16E91AF2`.

## Método

Dois **subagentes independentes** resolveram sete perguntas idênticas sobre C#, rotas, serialização Unity, interfaces, configuração e testes. Um usou a skill CodeGraph e suas queries por CLI; o outro usou apenas `rg` e leitura convencional de linhas. Nenhum recebeu resultados, contexto de resolução ou logs do outro. Ambos foram obrigados a instrumentar operações de leitura com `benchmarks/query_logger.py`, usando diretórios distintos. A revisão factual pelo agente principal foi externa à medição. Nenhuma alteração de código foi feita para este teste.

A métrica é `tiktoken/o200k_base` sobre o texto completo `stdout+stderr` das operações de leitura, incluindo retries, buscas vazias, mensagens de erro e leituras para verificar respostas da ferramenta semântica. Os **640 tokens da skill** foram incluídos uma vez em `S0.log`. Tokens de raciocínio, mensagens, parâmetros das ferramentas, custo de indexação e faturamento não são medidos. Capturas de código ficam no `target/tmp` ignorado pelo Git, separadas deste relatório agregado.

## Resultado

| Cenário | Leitura convencional | CodeGraph | Economia de saída |
|---|---:|---:|---:|
| S1. Métodos públicos e Unity de `UserProfilePanel` | 1.750 | 467 | **+73,3%** |
| S2. Permissões e ciclo de câmera de `BarcodeScannerWrapper` | 5.947 | 7.005 | −17,8% |
| S3. Rotas `WorldNavigationHelper` e referências de prefab | 7.836 | 2.320 | **+70,4%** |
| S4. Cache e erros em `DisplayScriptResolver` | 1.244 | 2.456 | −97,4% |
| S5. Contrato e implementações de `IDisplayScriptResolver` | 411 | 789 | −92,0% |
| S6. Dependências Firebase no manifesto Unity | 1.228 | 1.286 | −4,7% |
| S7. Referências em testes ao `DisplayScriptResolver` | 193 | 411 | −113,0% |
| **Total de consultas** | **18.609** | **14.734** | **+20,8%** |
| Skill CodeGraph (S0) | 0 | 640 | — |
| **Total incluindo a skill** | **18.609** | **15.374** | **+17,4%** |

**Economia ponderada com skill:** 17,384%. **Média aritmética dos sete percentuais de cenário (sem ratear a skill):** −25,880%. O critério de 20% **simultaneamente** em economia ponderada e média por cenário, definido em `benchmarks/README.md`, **não foi cumprido**. Nos dois cenários de maior volume de texto, CodeGraph poupou bastante; em cinco dos sete cenários o custo das operações do CodeGraph foi maior.

Foram **40 operações** do braço convencional (33 concluídas, cinco buscas válidas sem correspondência, duas falhas) e **64 operações** do CodeGraph (56 concluídas, duas buscas válidas sem correspondência, seis falhas). Destas últimas, **19 foram verificações convencionais** contabilizadas no próprio braço CodeGraph. Os tempos de execução somados das operações foram **0,922 s** e **19,161 s**, respectivamente. São tempos do medidor/processos, não o tempo completo dos agentes; o primeiro CodeGraph também inclui inicialização/atualização de índice.

## Qualidade e verificações

- **S1:** os dois braços localizaram três métodos públicos (`InviteUserAsFriend`, `AcceptUserInvite`, `RejectUserInvite`) e os métodos Unity privados `Start` e `OnDestroy`, todos em `UserProfilePanel.cs:28-178`. O esqueleto público do CodeGraph inicialmente omitiu métodos privados; o agente corrigiu com `include_private=true`.
- **S2:** ambos identificaram a distinção iOS/WebGL e Android no pedido de permissão, adiamento do `StartScanning` até a inicialização, tratamento de negação com `Debug.LogWarning`, pausa por `StopScanning` e parada definitiva da webcam em `OnDestroy` (`BarcodeScannerWrapper.cs:55-295`). O CodeGraph precisou consultar múltiplos trechos para cobrir todos os ramos.
- **S3:** `WorldNavigationHelper.cs:51-77` possui dois overloads `GotoArea`, um `GotoAreaByRef`, um `GotoAreaLocation` que chama `Scene.LoadLocation` e outro método `NavigateToAreaLocation` que usa `Routes.AreaLocation`. Fora da declaração, **não há chamadas C# textuais diretas** a `GotoAreaLocation` em `Assets/Game`; entretanto existem **ligações serializadas em prefabs Unity**, por exemplo `FireArea.prefab:272`, `GrasslandsArea.prefab:324`, `CellisMarketHome.prefab:159`. O CodeGraph teve de usar `rg` de fallback, pois a busca indexada de C# não cobre esse universo. Um binding serializado não garante invocação em runtime.
- **S4:** ambos cobriram as validações e a chave de cache `displayScriptId/assetBundleId/prefabName`, reutilização de `Task<GameObject>`, falhas que removem o cache e `ClearCache` em `DisplayScriptResolver.cs:32-129`. Um detalhe real é que exceções de seletor/load não são capturadas localmente e podem deixar uma task com falha em cache. O CodeGraph abriu corpos adicionais para cobrir helpers inicialmente ausentes do fluxo selecionado.
- **S5:** `IDisplayScriptResolver.cs:18-26` declara `ResolvePrefabAsync<T>` e `ClearCache`; `DisplayScriptResolver.cs:14` é a **única implementação direta** encontrada em `Assets/Game`. Instanciações pelo `FirebaseManager` não representam novos implementadores.
- **S6:** `Packages/manifest.json` e `Packages/packages-lock.json` não declaram pacotes Firebase; a busca em `Packages` também não retornou correspondências. Nomes de arquivos em `GooglePackages` ficam fora desse escopo. A ferramenta semântica informou corretamente que o manifesto não estava indexado; o fallback convencional foi necessário antes de afirmar ausência.
- **S7:** nenhuma referência direta a `DisplayScriptResolver` em arquivos C# de teste nos escopos especificados. `Tests` existe, mas não contém C#; `Assets/Game` contém ocorrências em produção. O CodeGraph assinalou que `Tests` não tinha arquivos indexados, e o agente conferiu por `rg` antes de concluir.

Os fatos centrais de S1–S7 são consistentes com a revisão independente. A investigação convencional foi mais profunda em detalhes de roteamento e contabilizou **31 ocorrências serializadas em dez prefabs** em S3; o braço CodeGraph citou exemplos e reconheceu a existência de bindings. Assim, a equivalência é adequada às perguntas formuladas, mas não representa cobertura exaustiva de todas as referências dinâmicas Unity.

## Erros e inconsistências

1. **Argumentos incorretos:** `get_file_skeleton(path=...)` em S1 falhou porque exige `file_path`; `get_flow_evidence(symbol=...)` falhou em S3 e S4 porque exige `symbol_id`. Todas as tentativas tiveram saída e custo registrados.
2. **Ferramenta inexistente:** a consulta CodeGraph `get_implementations` em S5 falhou. O agente recorreu ao esqueleto e a `rg` para confirmar implementações diretas.
3. **Limite do índice:** `search_source` em `Packages/manifest.json` (S6) e `Tests` (S7) retornou erro de escopo sem arquivos indexados. Isso **não significa zero correspondências**; as duas buscas negativas foram confirmadas com ferramentas convencionais, contabilizadas.
4. **Erros da leitura convencional:** buscas em caminhos inexistentes `Assets/Game/App/Scripts/UI/Routes.cs` (S3) e `Assets/Game/Tests` (S7) produziram exit 2, depois corrigidos. O agente também relatou rejeições de argumentos `--rg -i`/`--glob` pelo medidor antes da criação de registro; estes erros qualitativos **não estão** nos totais de tokens. Outras buscas `rg` exit 1 representam ausência válida, e foram classificadas como `no_matches`.
5. **Cobertura de parsing:** respostas CodeGraph indicaram **1.447 arquivos indexados**, dos quais **12 com decodificação UTF-8 com perdas** no escopo da busca. Não foi demonstrada divergência factual nesses arquivos nesta bateria.
6. **Concorrência no repositório Unity:** no início, `git status` mostrava só dois arquivos modificados em `ProjectSettings`; durante o teste surgiram remoções em `Assets/Game/AssetBundles/Areas/Grasslands/Data` e conteúdo novo em `Grasslands/Resources`. Os agentes trabalharam em modo leitura. A origem dessas alterações paralelas não foi estabelecida; preservei a árvore de trabalho, e a movimentação de assets limita a reprodutibilidade exata da amostra.

## Reprodução e limitações

Logs brutos: `target/tmp/bench_20261023_holdout/baseline` e `target/tmp/bench_20261023_holdout/cg`. Comparação gerada: `target/tmp/bench_20261023_holdout/comparison.json`. Comando: `python benchmarks/compare_reading.py --baseline target/tmp/bench_20261023_holdout/baseline --candidate target/tmp/bench_20261023_holdout/cg --output target/tmp/bench_20261023_holdout/comparison.json`.

Este é **um benchmark de uma execução por braço, em um único repositório Unity/C#**, e quantifica apenas texto devolvido pelas operações de leitura. Os resultados não provam economia de tokens de sessão, latência total, ganho estatístico ou comportamento semelhante em outras linguagens/projetos.
