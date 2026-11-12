# CodeGraph reading-token benchmark: Warcodes Mobile (Unity/C#)

**Date:** 2026-10-21. **Repository snapshot:** `f1db1937161a1b5b9a53ae727b6219d73d7acb90`. **Tokenizer:** `o200k_base` (tiktoken). **Metric:** actual recorded tool-response text, including stderr, CodeGraph skill (`S0.log`), verification reads and fallbacks. It does not measure hidden model tokens or API billing.

Two independent subagents answered the same seven code questions, with no communication or access to each other's results. One used the CodeGraph skill and CLI, the other used ordinary `rg` queries and targeted file reads. All source-reading results were captured by `benchmarks/query_logger.py`; comparison used `benchmarks/compare_reading.py`. The source project was read-only during the test.

| Scenario | Conventional tokens | CodeGraph tokens | Savings |
| --- | ---: | ---: | ---: |
| S1. Reroll controller: declaration, members and API | 7,190 | 5,041 | 29.9% |
| S2. Purchase popup: references and opening | 1,985 | 4,328 | -118.0% |
| S3. Purchase confirmation and subsequent upgrade callback | 4,999 | 2,950 | 41.0% |
| S4. Core proxy interface and implementation | 4,067 | 1,851 | 54.5% |
| S5. GlobalConfig definition and usage in World | 903 | 3,163 | -250.3% |
| S6. Test coverage for BattleModeSceneLoader | 959 | 783 | 18.4% |
| S7. Creature store UI-to-proxy workflow | 7,019 | 7,069 | -0.7% |
| **Scenarios only** | **27,122** | **25,185** | **7.1%** |
| CodeGraph skill instructions | 0 | 1,366 | — |
| **Total** | **27,122** | **26,551** | **2.1%** |

**Arithmetic mean of per-scenario savings: -32.2%.** The weighted mean across all tokens is **+2.1%**. Neither meets the benchmark harness's 20% criterion. Results are for this Unity repository, one run per arm, with adaptive reading paths; they do not establish universal efficacy.

The conventional agent made **56** logged reading operations (27 `rg`, 29 `read`); the CodeGraph agent made **79** (61 semantic queries, 17 `rg` verifications, 1 targeted `read`). Recorded invocation durations sum to 59.445 s and 152.408 s respectively, with an additional **unlogged 120 s timeout** for CodeGraph. These sums are not end-to-end wall times because some queries overlapped.

**Correctness review:** Both arms correctly identified the target types, core flows, one implementer, zero `GlobalConfig` references in the specified World directory, and zero directly linked tests. The CodeGraph S3 answer stops at the proxy interface without naming the concrete Firebase function identified in the conventional answer; CodeGraph S7 maps listing and search but omits the separate purchase operation via `CreatureProxy.Purchase`. Thus the semantic core is consistent, but detailed answer completeness is **not strictly equivalent** in S3/S7. Treat the measured 2.1% as an observed output-token comparison, **not a quality-adjusted saving**.

## Findings

- The CodeGraph arm experienced **four SQLite `database is locked` failures** while several CLI processes opened or refreshed the same index concurrently (S3, S5, S6, S7). Concurrent indexing is the likely cause, although a precise SQLite lock trace was not collected. These outputs were retained and charged. Subsequent semantic operations were run sequentially.
- The initial attempt for S1 **timed out at 120 seconds** while the executable indexed on startup. `query_logger.py` does not persist a `TimeoutExpired` event in its manifest, so this incident is documented here but has no output-token charge. Another initial query (S2) completed in **116.445 seconds**; a warm follow-up (S4 search) finished in **199 ms**. Setup and indexing time is separate from the reading-token metric.
- Small initial `max_token_budget` settings truncated search output (e.g. S1, S2, S4). Increasing budget and verifying original C# resolved missing information but consumed additional tokens.
- Several CLI calls returned actionable **argument errors or overloaded-symbol ambiguity** (S2, S3, S4). The method hierarchy expects `direction=callers|callees|both`, not `incoming|outgoing`; `get_type_contract` expects `type_id`, while `get_type_hierarchy` expects `symbol_id`. Overloaded methods require a more specific identifier. Those are request/schema errors, not by themselves broken indexing; all returned text remains in the logs.
- **Confirmed overload-resolution inconsistency (S2):** the hierarchy for the `List<ItemDataRef>` overload of `MainMenu.ShowPurchaseConfirmationPanel` adds a direct caller at `CreatureCoreTweenController.cs:777`. The fourth argument is `ICreatureUpgradeProxy.upgradeRarityCost`, declared `ItemDataRef` (`ICreatureUpgradeProxy.cs:33`); the directly applicable overload is the singular `ItemDataRef` method at `MainMenu.cs:1130-1137`, which then delegates to the list overload at `:1137`. The extra direct list-overload edge does not match C# overload resolution in this code. This may inflate a call hierarchy even when `exact` provenance is displayed; callers should review overload-sensitive edges.
- The `GlobalConfig` type is a Unity `ScriptableObject`; the CodeGraph `get_config_matrix` tool models supported config-key sources, not arbitrary ScriptableObject fields. The CodeGraph arm performed extra searches and conventional verification for an absence in `Assets/Game/App/Scripts/World`.
- The test query reported zero directly linked tests for `BattleModeSceneLoader`, consistent with independent conventional searches under `Assets` and `Tests`. A nearby mock is not proof of a test exercising the loader.
- Unity prefab/inspector bindings and remotely invoked Firebase functions restrict what static call edges alone can prove. The store panel reaches store operations indirectly through views, listing components and `GameInstance.CreatureStore`; the purchase action uses `CreatureProxy.Purchase()`.

The conventional arm had eight nonzero statuses, all legitimate `rg` no-match results. The CodeGraph arm had fourteen nonzero statuses: four lock failures, five semantic-query argument/ambiguity responses, and five `rg` no-match verification results. Its initial timeout was not captured by the logger.

## Reproduction and artifacts

The raw project-specific outputs and agent answers are retained locally, outside git, under:

`D:/Entidades/EmergentsWorlds/warcodes-mobileapp/Temp/CodeGraphBenchmarks/2026-10-21/`

The directories `baseline/` and `codegraph/` each contain `operations.jsonl`, per-scenario output logs and agent notes. The CodeGraph arm also contains a machine-readable `report.json` describing its operation inventory and the unlogged timeout. The sibling `comparison.json` contains the exact tokenizer counts; an aggregate copy is retained next to this report as `2026-10-21-warcodes-mobileapp.json`. Since raw logs may contain private source code, only aggregate results are tracked in the CodeGraph repository.

This is a real-world cold-to-warm test including recovery effort. The first invocation incurred index refresh costs; the full token comparison counts all visible read responses but not silent waiting. The CodeGraph arm made more queries than the baseline, including retries after errors. All seven tasks were investigated; missing results in restricted scopes were explicitly verified.
