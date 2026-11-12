# Warcodes mobile: independent reading-token benchmark, iteration 03

**Date:** 2026-10-21. **Source:** Warcodes Unity/C# repository, same seven tasks as `2026-10-21-warcodes-mobileapp.md`. **Tokenizer:** `o200k_base`. **Measurement:** actual captured tool-output tokens, including CodeGraph skill, error results, verification reads, and recovery. No model input, reasoning or API billing included.

| Scenario | Conventional | CodeGraph | Saving |
| --- | ---: | ---: | ---: |
| S1 Reroll controller | 6,930 | 2,225 | +67.9% |
| S2 Purchase popup references | 888 | 1,985 | -123.5% |
| S3 Confirmation callback to backend | 2,894 | 5,727 | -97.9% |
| S4 Core proxy API | 2,577 | 233 | +91.0% |
| S5 Scoped GlobalConfig | 1,105 | 1,607 | -45.4% |
| S6 Tests for battle loader | 973 | 1,138 | -17.0% |
| S7 Marketplace UI to proxy | 6,407 | 9,965 | -55.5% |
| **Total (skill included)** | **21,774** | **24,290** | **-11.6%** |

**Arithmetic mean of scenario savings: -25.8%. Neither the 20% weighted nor 20% arithmetic target passed.** Both agents were isolated and produced independent source-location answers. The CodeGraph arm captured 90 operations; baseline captured 50. Baseline's S2/S3 work reuses already-seen within-arm context, which shifts per-scenario costs; weighted aggregate is more robust to this issue. The agent's decisions are variable, so a single execution does not estimate universal savings.

Semantic evidence agreed on core findings, but the CodeGraph agent used many separate `get_symbol_body` operations to follow callbacks, Unity UI transitions and backend requests. S3 (25 calls) and S7 (28 calls) dominated overhead. Exact false links between purchase-popup overloads were gone, but the singular C# overload's caller still appeared unresolved because some lambda argument types were unknown. These issues informed *generic* improvements: conservative partial-type overload selection and `get_flow_evidence`, a compact, explicitly incomplete excerpt of control statements and call sites supported on indexed languages. Subsequent tests and holdouts are documented separately; their economy must be measured independently.

**Instrumentation issue:** the baseline's concurrent invocations lost one `operations.jsonl` entry although its complete 738-byte output was present in S2.log. The agent recovered the entry transparently, without rerunning the search, and reconciled all log byte counts. The recording utility was subsequently hardened with an interprocess lock and a stress test of 64 processes.

Raw code and transcripts stay under the Warcodes repository's git-ignored `Temp/CodeGraphBenchmarks/2026-10-21/iteration-03/` directory. They are not copied into the CodeGraph source tree.
