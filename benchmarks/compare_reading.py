"""Compare benchmark arms using captured text, not estimated model billing.

Usage:
  python benchmarks/compare_reading.py \
    --baseline benchmarks/example/baseline \
    --candidate benchmarks/example/codegraph \
    --output benchmarks/example/comparison.json

Accepts UTF-8 and UTF-16LE files emitted by PowerShell Tee-Object.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def decode(path: Path) -> str:
    raw = path.read_bytes()
    if raw.startswith((b"\xff\xfe", b"\xfe\xff")):
        return raw.decode("utf-16")
    if raw.startswith(b"\xef\xbb\xbf"):
        return raw.decode("utf-8-sig")
    if raw and raw[1::2].count(0) > len(raw[1::2]) // 3:
        return raw.decode("utf-16-le")
    return raw.decode("utf-8", errors="replace")


def count_arm(directory: Path, tokenizer) -> dict:
    result = {"main": {}, "verify": {}, "skill": 0, "files": {}}
    for scenario in range(8):
        for suffix, group in ((".log", "main"), (".verify.log", "verify")):
            path = directory / f"S{scenario}{suffix}"
            if not path.is_file():
                continue
            token_count = len(tokenizer.encode(decode(path), disallowed_special=()))
            result["files"][path.name] = token_count
            if scenario == 0:
                result["skill"] += token_count
            else:
                result[group][str(scenario)] = token_count
    result["by_scenario"] = {
        str(n): result["main"].get(str(n), 0) + result["verify"].get(str(n), 0)
        for n in range(1, 8)
    }
    result["total"] = sum(result["by_scenario"].values()) + result["skill"]
    result["missing_scenarios"] = [
        n for n in range(1, 8)
        if f"S{n}.log" not in result["files"]
    ]
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    opts = parser.parse_args()
    try:
        import tiktoken
    except ImportError as exc:
        raise SystemExit("Install benchmark dependencies: python -m pip install -r benchmarks/requirements.txt") from exc

    tokenizer = tiktoken.get_encoding("o200k_base")
    baseline = count_arm(opts.baseline, tokenizer)
    candidate = count_arm(opts.candidate, tokenizer)
    weighted_savings = (
        1 - candidate["total"] / baseline["total"]
        if baseline["total"] else 0
    )
    scenario_savings = {
        str(i): 1 - candidate["by_scenario"][str(i)] / baseline["by_scenario"][str(i)]
        if baseline["by_scenario"][str(i)] else None
        for i in range(1, 8)
    }
    average_savings = sum(
        s for s in scenario_savings.values() if s is not None
    ) / sum(s is not None for s in scenario_savings.values())
    result = {
        "tokenizer": "o200k_base",
        "measurement": "captured tool-output tokens + skill and verification logs",
        "quality_gate": "REQUIRES_INDEPENDENT_REVIEW",
        "baseline": baseline,
        "candidate": candidate,
        "scenario_savings": scenario_savings,
        "weighted_savings": weighted_savings,
        "arithmetic_mean_scenario_savings": average_savings,
        "weighted_target_met": weighted_savings >= 0.20 and not candidate["missing_scenarios"],
        "scenario_mean_target_met": average_savings >= 0.20 and not candidate["missing_scenarios"],
    }
    opts.output.parent.mkdir(parents=True, exist_ok=True)
    opts.output.write_text(json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8")
    for i in range(1, 8):
        print(f"S{i}: {baseline['by_scenario'][str(i)]:6} -> {candidate['by_scenario'][str(i)]:6} "
              f"({scenario_savings[str(i)]:+.1%} savings)")
    print(f"Totals: {baseline['total']} -> {candidate['total']} "
          f"({weighted_savings:+.1%} weighted savings, {average_savings:+.1%} unweighted mean)")
    if candidate["missing_scenarios"]:
        print("MISSING", candidate["missing_scenarios"])


if __name__ == "__main__":
    main()
