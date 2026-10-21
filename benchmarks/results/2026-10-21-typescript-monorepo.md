# CodeGraph reading-token benchmark: TypeScript monorepo

**Date:** 2026-10-21. **Metric:** `o200k_base` tokens from captured tool outputs, including instructions and any verification/fallback reads. This is not total model usage.

An independent pair of agents investigated seven equivalent source-reading tasks, using conventional text tools and CodeGraph respectively. The underlying repository is private; detailed transcripts, source excerpts and project identifiers are deliberately excluded from this public repository.

| Scenario | Conventional reads | CodeGraph reads | Saving |
| --- | ---: | ---: | ---: |
| Authentication | 1,563 | 1,459 | 6.7% |
| All call sites | 2,692 | 1,666 | 38.1% |
| Backend route to database | 3,277 | 3,426 | -4.5% |
| Public API | 2,003 | 854 | 57.4% |
| Environment configuration | 4,806 | 5,437 | -13.1% |
| Tests for a symbol | 6,137 | 1,599 | 73.9% |
| Frontend to backend | 4,234 | 3,560 | 15.9% |
| **Total** | **24,712** | **19,443** | **21.3%** |

Arithmetic mean of scenario savings: **24.9%**. The conventional arm came from the second independent execution, and the CodeGraph arm from the fourth execution after generic improvements. A separate original baseline consumed 33,889 tokens, underscoring agent-to-agent variation.

**Limitations:** one project, one execution per arm, and adaptive agent strategies. Improvements were also checked on synthetic TypeScript, JavaScript, Python, Rust and C# fixtures. The measured result satisfies the local 20% target but is not evidence of universal savings. Remaining issues include conditional branches over-attributed by a generic route tracer, unresolved database operations through dynamic callback receivers, and behaviorally covered errors lacking direct test edges.
