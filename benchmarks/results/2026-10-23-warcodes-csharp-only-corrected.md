# Benchmark corrigido — Warcodes Mobile, somente arquivos `.cs`

**Data:** 23/10/2026. **Código-fonte:** `D:/Entidades/EmergentsWorlds/warcodes-mobileapp`, Git HEAD `00b453eb43c0dee6dff8c4194b4464c65b01c47e`. **CodeGraph:** Git HEAD `a42e9722114715afb8013072450f2738278b81b9`, binário SHA-256 `076010B0A51A61F97E1150B49061BBC5A76A0E545991948B2A185B7F16E91AF2`.

## Correção de escopo

Na execução anterior, o cenário S3 foi ampliado indevidamente para arquivos Unity `.prefab` a fim de procurar ligações serializadas de `GotoAreaLocation`. Os testes solicitavam leitura de **código C#**, de modo que esses prefabs eram irrelevantes. O cenário anterior S6 também lia `Packages/manifest.json`, incompatível com o novo escopo exclusivamente `.cs`. Esta execução é uma bateria nova de **sete cenários C#** com S3 estritamente textual/semântico e S6 substituído por inicialização de `DisplayScripts` em `FirebaseManager.cs`. A comparação é feita entre agentes **desta execução**, não por subtração de operações antigas.

Dois novos subagentes isolados receberam perguntas idênticas e proibiram mutuamente o uso de outputs, conclusões e logs do outro. O braço convencional usou buscas `rg` com filtro de arquivo `--glob '*.cs'` (além de leituras `.cs`). O CodeGraph usou apenas queries semânticas sobre C# e fallbacks instrumentados estritamente em `.cs`. Todos os comandos que leem conteúdo passaram por `benchmarks/query_logger.py`. A auditoria de operações encontrou **zero leituras de arquivos não C#** em ambos os braços. Os resultados são contagem `tiktoken/o200k_base` de stdout + stderr, incluindo erros, retries e verificações. O texto da skill foi contabilizado uma vez em S0 (682 tokens).

## Resultado observado

| Cenário C# | Leitura convencional | CodeGraph | Economia de saída |
|---|---:|---:|---:|
| S1. Assinaturas e lifecycle de `UserProfilePanel` | 1.751 | 394 | +77,5% |
| S2. Permissões e scanner `BarcodeScannerWrapper` | 5.759 | 6.620 | −15,0% |
| S3. `WorldNavigationHelper` e chamadas C# | 801 | 1.040 | −29,8% |
| S4. Cache de `DisplayScriptResolver` | 1.244 | 2.451 | −97,0% |
| S5. Interface `IDisplayScriptResolver` | 358 | 237 | +33,8% |
| S6. Inicialização Firebase em C# | 1.413 | 2.201 | −55,8% |
| S7. Referências em testes C# | 193 | 411 | −113,0% |
| **Consultas, sem skill** | **11.519** | **13.354** | **−15,9%** |
| Skill CodeGraph | 0 | 682 | — |
| **Total com skill** | **11.519** | **14.036** | **−21,9%** |

**Economia ponderada incluindo skill:** **−21,85%**. **Média simples das economias por cenário:** **−28,46%**. O CodeGraph consumiu **2.517 tokens adicionais**, portanto **não atingiu** o limiar de economia de 20% neste recorte. A restrição a `.cs` removeu parte expressiva do custo do braço convencional em S3.

Foram **20 operações convencionais** (16 concluídas, três buscas válidas sem matches, uma falha de caminho) e **61 operações CodeGraph** (57 concluídas, duas buscas válidas sem matches, duas falhas). A soma dos tempos registrados por operação foi de **220 ms** versus **9.402 ms**, respectivamente; isso exclui tempo de raciocínio e latência da sessão do agente.

## Conferência factual

- **S1:** os três métodos públicos de `UserProfilePanel` estão nas linhas 126, 152 e 178 e `Start`/`OnDestroy` nas linhas 28/49 do mesmo `.cs`.
- **S2:** ambos os braços identificaram pedidos de permissão iOS/WebGL e Android, tratamento de negação, inicialização da câmera, `StartScanning` e `StopScanning` em `BarcodeScannerWrapper.cs`. CodeGraph expandiu muitos métodos para cobrir os ramos.
- **S3:** `WorldNavigationHelper.cs:51-77` define `GotoArea(string)`, `GotoArea(AreaData)`, `GotoAreaByRef`, `GotoAreaLocation` e `NavigateToAreaLocation`. **Nenhuma chamada direta adicional a `GotoAreaLocation` aparece em `.cs` sob `Assets/Game`**. Nenhuma ligação de prefab integra este benchmark.
- **S4:** `DisplayScriptResolver.cs:16-129` contém cache de tasks, chave combinada, validação de ID/tipo/referência, descarte de entradas em falhas retornando null e limpeza explícita. Exceções sem tratamento local podem manter uma task com falha no cache.
- **S5:** interface `IDisplayScriptResolver.cs:18-26` contém `ResolvePrefabAsync<T>` e `ClearCache`. Único implementador direto encontrado: `DisplayScriptResolver.cs:14`.
- **S6:** `FirebaseManager.cs:338` chama `InitializePromotionalOffers`; na compilação de editor com mock válido atribui `new DisplayScriptResolver(mockData)` (:378). Nos demais caminhos, inclusive mock sem dados, cria o resolver padrão (:391).
- **S7:** não foram identificados arquivos de teste C# com o texto `DisplayScriptResolver` em `Assets/Game` ou `Tests`. Uma ausência no índice sob `Tests` foi verificada com busca convencional filtrada em `.cs`.

## Falhas e limitações

A consulta CodeGraph `get_symbol_body` usando a assinatura abreviada `WorldNavigationHelper.GotoArea/1` retornou erro em S3. Outra consulta `search_source` em `Tests` retornou erro de escopo não indexado em S7, que foi resolvido por fallback convencional contado. No braço convencional, uma busca em `Assets/Game/Tests` falhou por caminho inexistente; a ausência foi conferida em `Assets/Game` e `Tests` com buscas `.cs` válidas. Todas essas operações registradas compõem o custo medido.

A mudança de escopo torna este resultado **não comparável diretamente** ao benchmark anterior, que incluía prefab e manifesto JSON. Os dados medem somente texto recebido de ferramentas, não tokens totais de modelo ou faturamento. Houve uma execução por braço, em um projeto C#/Unity; não é prova de desempenho universal nem revisão dinâmica do jogo. Alterações anteriores da árvore de trabalho foram preservadas.

**Logs privados locais:** `target/tmp/bench_20261023_cs_only_02/{baseline,cg}` (incluem trechos de código). **Comparador:** `target/tmp/bench_20261023_cs_only_02/comparison.json`. Execute `python benchmarks/compare_reading.py --baseline target/tmp/bench_20261023_cs_only_02/baseline --candidate target/tmp/bench_20261023_cs_only_02/cg --output target/tmp/bench_20261023_cs_only_02/comparison.json` para reproduzir a contagem.
