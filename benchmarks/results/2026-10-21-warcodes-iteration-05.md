# Warcodes reading-token benchmark: iteration 05 (compact output)

**Date:** 2026-10-21. **Protocol:** Seven identical source-reading tasks, independently investigated by an isolated CodeGraph agent and the frozen conventional baseline from iteration 03. Tokenizer: `o200k_base`, measuring captured tool output and verification, including skill instructions and failed calls.

| Scenario | Conventional | CodeGraph | Saving |
| --- | ---: | ---: | ---: |
| S1 Reroll controller | 6,930 | 2,212 | +68.1% |
| S2 Popup direct references and callers | 888 | 1,847 | -108.0% |
| S3 Confirmation to backend callback | 2,894 | 5,731 | -98.0% |
| S4 Proxy type API | 2,577 | 1,018 | +60.5% |
| S5 Scoped GlobalConfig search | 1,105 | 499 | +54.8% |
| S6 Battle loader tests | 973 | 1,108 | -13.9% |
| S7 Marketplace UI flow | 6,407 | 8,601 | -34.2% |
| **Total, including instructions** | **21,774** | **21,386** | **+1.8%** |

**Arithmetic mean of seven scenario savings: -10.1%. The 20% target was not met.** This was a complete run, including 87 CodeGraph tool operations and the agent's finalized source-evidence answers. Explicit `format=json` was no longer injected by the recorder; the agent used each query tool's compact default. Some workflows still re-read many individual methods to verify asynchronous handlers and mutations. Three overload ambiguity errors and a nonexistent/unindexed tests scope were recorded, rather than excluded.

The following subsequent improvements are language-agnostic: `get_flow_evidence` now includes assignment candidates alongside control/call lines and supports grouping by type; `get_file_outline` accepts a member-name filter; `get_call_hierarchy` and `get_flow_evidence` can resolve a family of same-qualified callable overloads in one request. The benchmark above was executed before the latter improvements and therefore does not measure their benefit.

Raw source excerpts and agent notes remain under `warcodes-mobileapp/Temp/CodeGraphBenchmarks/2026-10-21/iteration-05/codegraph/`, ignored by git. This is one realization of adaptive agent behavior; it does not provide an inference about universal token savings.
