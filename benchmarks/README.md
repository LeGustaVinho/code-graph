# Benchmarking CodeGraph read-token efficiency

The benchmark compares the text returned by reading tools, including CodeGraph
verification fallbacks and instructions. It is not a measure of hidden model
reasoning or API billing. Both arms should answer **identical** code questions,
record source locations, and be checked for equivalent correctness before a
token-saving claim is accepted.

## Protocol

1. Choose a repository snapshot and seven or more representative tasks spanning
   symbol discovery, all call sites, routes, config, tests, and cross-module flow.
   Prefer additional repositories and languages as independent holdouts.
2. Give the **same tasks** to isolated agents; the conventional arm can use text
   searches/line ranges, and the CodeGraph arm can query the graph and fall back
   to recorded conventional reads. Neither reads the other's outputs.
3. Instrument **every code-reading operation** using `query_logger.py`: it
   records stdout, stderr, arguments, status, and durations. Record the
   CodeGraph skill in `S0.log`, and use `--verify` for text reads performed
   while validating CodeGraph answers.
4. Compare the logs using `compare_reading.py` (o200k_base). Require **both**
   the weighted savings across total tokens and arithmetic mean scenario
   savings to exceed 20%, with complete logs and manual equivalence review.
5. Run multi-language semantic fixtures via `holdout_smoke.py` and Rust tests
   to reject optimizations that only improve one application's structure.

## Commands

From the CodeGraph repo, in PowerShell:

```powershell
python -m pip install -r benchmarks/requirements.txt
python benchmarks/query_logger.py --run benchmarks/example/codegraph --scenario S1 --root D:/repo --cg search_universal --arg query=Order
python benchmarks/query_logger.py --run benchmarks/example/codegraph --scenario S1 --root D:/repo --read src/order.ts --start 40 --end 60 --verify
python benchmarks/query_logger.py --run benchmarks/example/baseline --scenario S1 --root D:/repo --rg 'Order' --path src
python benchmarks/holdout_smoke.py target/release/codegraph.exe
python benchmarks/compare_reading.py --baseline benchmarks/example/baseline --candidate benchmarks/example/codegraph --output benchmarks/example/comparison.json
```

### Interpretation

Reading fewer tokens with a wrong or incomplete answer is **not** a successful
optimization. A low token budget that truncates needed evidence must be
increased and the extra output included in the total. A CodeGraph agent's
fallback text reads are **part of its cost**. Setup/indexing effort and execution
time should be reported separately, since this metric measures tool-output
tokens. Count skill-loading cost when included in the agent's context.

Raw benchmark captures from private repositories are kept locally and excluded
from the public source tree to avoid publishing third-party source code.
Only aggregate results with no copied source are committed (see `results/`).
The recorder, comparator, and holdout script are general and parameterized.
A single agent run per arm is not statistical evidence of universal savings.
