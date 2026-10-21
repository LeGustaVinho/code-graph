"""Record *exact* stdout presented to a code-reading benchmark participant.

Examples (PowerShell, from the CodeGraph repository):

  python benchmarks/query_logger.py --run benchmarks/example/codegraph --scenario S1 \
    --root D:/repo --cg get_file_skeleton --arg file_path=src/main.ts
  python benchmarks/query_logger.py --run benchmarks/example/codegraph --scenario S1 \
    --root D:/repo --read src/main.ts --start 40 --end 65 --verify
  python benchmarks/query_logger.py --run benchmarks/example/baseline --scenario S1 \
    --root D:/repo --rg "function actor" --path src

The script does not trim outputs; any display truncation imposed by the calling
host must be noted by the participant. stdout and stderr are kept separate.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


def parse_value(raw: str):
    try:
        return json.loads(raw)
    except ValueError:
        return raw


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--scenario", choices=[f"S{i}" for i in range(8)], required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/codegraph.exe")
    parser.add_argument("--cg", metavar="TOOL")
    parser.add_argument("--arg", action="append", default=[], metavar="KEY=VALUE")
    parser.add_argument("--read", metavar="PATH")
    parser.add_argument("--start", type=int, default=1)
    parser.add_argument("--end", type=int)
    parser.add_argument("--rg", metavar="REGEX")
    parser.add_argument("--path", default=".")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()

    modes = [bool(args.cg), bool(args.read), args.rg is not None]
    if sum(modes) != 1:
        parser.error("choose exactly one of --cg, --read, --rg")
    root = args.root.resolve()
    if not root.is_dir():
        parser.error(f"missing project root: {root}")
    args.run.mkdir(parents=True, exist_ok=True)
    t0 = time.perf_counter()

    if args.cg:
        kwargs = {"format": "json", "max_token_budget": 1800}
        for pair in args.arg:
            if "=" not in pair:
                parser.error("--arg requires KEY=VALUE")
            key, raw = pair.split("=", 1)
            kwargs[key] = parse_value(raw)
        cmd = [
            str(args.binary.resolve()), "query", args.cg, "--root", str(root),
            "--args", json.dumps(kwargs, ensure_ascii=False),
        ]
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=root, timeout=120)
        data = proc.stdout
        error = proc.stderr
    elif args.read:
        target = (root / args.read).resolve()
        if not target.is_file() or not target.is_relative_to(root):
            parser.error(f"invalid source path: {args.read}")
        lines = target.read_text(encoding="utf-8-sig", errors="replace").splitlines()
        end = min(len(lines), args.end or len(lines))
        data = ("\n".join(f"{i + 1}| {lines[i]}" for i in range(max(0, args.start - 1), end)) + "\n").encode()
        error = b""
        proc = subprocess.CompletedProcess(["read", str(target)], 0)
    else:
        cmd = ["rg", "-n", "--no-heading", args.rg, args.path]
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=root, timeout=90)
        data = proc.stdout
        error = proc.stderr

    filename = args.scenario + (".verify.log" if args.verify else ".log")
    path = args.run / filename
    with path.open("ab") as handle:
        handle.write(data)
        if data and not data.endswith(b"\n"):
            handle.write(b"\n")
        if error:
            handle.write(error)
            if not error.endswith(b"\n"):
                handle.write(b"\n")
    manifest = {
        "utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "scenario": args.scenario,
        "mode": "codegraph" if args.cg else ("read" if args.read else "rg"),
        "tool": args.cg or args.read or args.rg,
        "arguments": dict(
            cg_args=args.arg, root=str(root), verify=args.verify,
            line_range=[args.start, args.end] if args.read else None,
            search_path=args.path if args.rg is not None else None,
        ),
        "exit": proc.returncode,
        "stdout_bytes": len(data),
        "stderr_bytes": len(error),
        "log": filename,
        "duration_ms": round((time.perf_counter() - t0) * 1000),
    }
    with (args.run / "operations.jsonl").open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(manifest, ensure_ascii=False) + "\n")
    sys.stdout.buffer.write(data)
    sys.stderr.buffer.write(error)
    sys.exit(proc.returncode)


if __name__ == "__main__":
    main()
