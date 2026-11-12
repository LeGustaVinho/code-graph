# Warcodes reading-token benchmark: iteration 04 (new flow excerpts)

**Date:** 2026-10-21. **Comparison:** same independent conventional S1–S7 baseline as iteration 03, newly isolated CodeGraph agent on the rebuilt x12 index. All queries and fallback reads captured; CodeGraph skill loading is included. Tokens use `o200k_base`.

| Scenario | Conventional tokens | CodeGraph tokens | Saving |
| --- | ---: | ---: | ---: |
| S1 Reroll controller | 6,930 | 2,498 | +64.0% |
| S2 Purchase confirmation popup | 888 | 3,210 | -261.5% |
| S3 Confirmation through backend | 2,894 | 5,135 | -77.4% |
| S4 Core proxy interface | 2,577 | 1,037 | +59.8% |
| S5 Scoped GlobalConfig | 1,105 | 656 | +40.6% |
| S6 Tests for battle loader | 973 | 1,704 | -75.1% |
| S7 Store UI to proxy | 6,407 | 8,506 | -32.8% |
| **Total, skill included** | **21,774** | **24,263** | **-11.4%** |

Arithmetic mean of per-scenario savings: **-40.4%**. **Target not reached.** The CodeGraph agent made 93 tool queries (92 semantic, one conventional verification), with two nonzero operations. It reproduced the direct singular-overload caller correctly after the C# type-aware resolver fix. A syntactically missing `Assets/Tests` search path was explicitly reported, as was one C# file decoded lossily. Unity inspector bindings and remote function internals remained outside statically provable edges.

The new `get_flow_evidence` reduced some individual reads, but the agent still requested a very large unfiltered outline and many separate method bodies for the callback and marketplace flows. As a benchmark instrumentation issue, the recorder forced `format=json` on CodeGraph outputs during this run, despite the CLI's more economical compact default. Later iterations remove this forced argument and introduce a language-independent declaration filter for outlines and a type-level flow excerpt. These changes must be judged on a *new* run; this result has not been overwritten.

Raw logs and answers are retained under `warcodes-mobileapp/Temp/CodeGraphBenchmarks/2026-10-21/iteration-04/codegraph/`. They are git-ignored and not committed with this aggregate report.
