"""Independent small-project smoke test of CodeGraph navigation quality.

This is a multi-language holdout smoke check with no application-specific data
or expected answers. All indexing runs on temporary copies of synthetic fixtures.

Usage: python benchmarks/holdout_smoke.py [path/to/codegraph.exe]
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
EXECUTABLE = (
    Path(sys.argv[1]).resolve()
    if len(sys.argv) > 1
    else ROOT / "target" / "debug" / "codegraph.exe"
)
TOKEN_BUDGET = 1500


def run(*args: str) -> str:
    completed = subprocess.run(
        [str(EXECUTABLE), *args],
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=90,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Command {' '.join(args)} failed ({completed.returncode}): "
            f"{completed.stdout[-1500:]} {completed.stderr[-1500:]}"
        )
    return completed.stdout


def query(root: Path, tool: str, **args) -> str:
    # Passing separate argv fields avoids shell quoting corruption.
    args.setdefault("max_token_budget", TOKEN_BUDGET)
    args.setdefault("format", "json")
    return run("query", tool, "--root", str(root), "--args", json.dumps(args))


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="codegraph-holdout-") as temp:
        base = Path(temp)
        datasets = {
            "typescript-python": ROOT / "fixtures" / "edge-cases" / "closures",
            "rust": ROOT / "fixtures" / "edge-cases" / "rust",
            "csharp": ROOT / "fixtures" / "edge-cases" / "csharp",
        }
        for name, source in datasets.items():
            dest = base / name
            shutil.copytree(source, dest, ignore=shutil.ignore_patterns(".codegraph"))
            index = run("index", str(dest), "--json")
            print(f"indexed {name}: {index[:200].strip()}")

        ts = base / "typescript-python"
        checks = [
            (
                "nested TS helper symbol",
                query(ts, "search_universal", query="buildApp.actor"),
                ["buildApp.actor", "app.ts"],
            ),
            (
                "TS inherited method with repeated call sites",
                query(ts, "get_call_hierarchy", symbol_id="BaseService.rate", direction="callers", depth=1),
                ["AppService.me", "@30,31"],
            ),
            (
                "TS route through registration wrapper",
                query(ts, "trace_fullstack_flow", api_route_or_call="POST /v1/friends", depth=2),
                ["/v1/friends", "AppService.friend"],
            ),
            (
                "TS environment aliases",
                query(ts, "get_config_matrix", filter="DATABASE_URL"),
                ["DATABASE_URL", "config.ts"],
            ),
            (
                "nested Python helper",
                query(ts, "search_universal", query="create_app.load_user"),
                ["create_app.load_user", "factory.py"],
            ),
        ]
        for name, output, expected in checks:
            for substring in expected:
                assert substring in output, f"{name}: {substring!r} missing in {output[:1600]!r}"
            print(f"PASS {name} ({len(output)} chars)")

        for name in ("rust", "csharp"):
            project = base / name
            output = query(project, "get_entrypoints", max_token_budget=400)
            assert len(output) < 6000, f"{name}: output budget ignored ({len(output)} chars)"
            search = query(project, "search_universal", query="test", max_token_budget=350)
            assert len(search) < 6000, f"{name}: search output budget ignored ({len(search)} chars)"
            print(f"PASS {name} bounded navigation ({len(output)} / {len(search)} chars)")


if __name__ == "__main__":
    main()
