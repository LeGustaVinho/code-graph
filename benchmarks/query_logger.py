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
from contextlib import contextmanager
import errno
from fnmatch import fnmatch
import json
import os
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


def _bytes(value: bytes | str | None) -> bytes:
    if value is None:
        return b""
    return value if isinstance(value, bytes) else value.encode("utf-8", errors="replace")


def run_subprocess(cmd: list[str], root: Path, timeout: int) -> tuple[int, bytes, bytes, str, str | None]:
    """Preserve partial output and report subprocess failures as benchmark operations."""
    try:
        proc = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=root, timeout=timeout)
        status = "completed" if proc.returncode == 0 else "nonzero_exit"
        failure = None if proc.returncode == 0 else f"subprocess exited with status {proc.returncode}"
        return proc.returncode, proc.stdout, proc.stderr, status, failure
    except subprocess.TimeoutExpired as exc:
        detail = f"subprocess timed out after {timeout}s: {exc}"
        partial = _bytes(exc.stderr)
        stderr = partial + (b"\n" if partial and not partial.endswith(b"\n") else b"") + f"[query_logger] {detail}\n".encode()
        return 124, _bytes(exc.stdout), stderr, "timeout", detail
    except (OSError, subprocess.SubprocessError) as exc:
        detail = f"subprocess failed ({type(exc).__name__}): {exc}"
        partial = _bytes(getattr(exc, "stderr", None))
        stderr = partial + (b"\n" if partial and not partial.endswith(b"\n") else b"") + f"[query_logger] {detail}\n".encode()
        return 127 if isinstance(exc, OSError) else 125, _bytes(getattr(exc, "stdout", None)), stderr, "subprocess_error", detail


_CAPTURE_LOCK_TIMEOUT_SECONDS = 30
_CAPTURE_LOCK_POLL_SECONDS = 0.02


@contextmanager
def serialized_capture(run: Path):
    """Serialize log+manifest appends across independent CLI processes.

    Opening a file in append mode alone does not make two separate writes to
    different files atomic. Keep the lock file: the OS releases the lock when
    the process exits, and a bounded wait avoids hanging on a stalled writer.
    """
    with (run / ".capture.lock").open("a+b") as lock_file:
        if os.name == "nt":
            import msvcrt

            def acquire():
                # A Windows byte-range lock may extend beyond EOF; no sentinel
                # write is needed before taking the lock on a new, empty file.
                lock_file.seek(0)
                msvcrt.locking(lock_file.fileno(), msvcrt.LK_NBLCK, 1)

            def release():
                lock_file.seek(0)
                msvcrt.locking(lock_file.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            def acquire():
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

            def release():
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)

        deadline = time.monotonic() + _CAPTURE_LOCK_TIMEOUT_SECONDS
        while True:
            try:
                acquire()
                break
            except OSError as exc:
                if exc.errno not in (errno.EACCES, errno.EAGAIN, errno.EWOULDBLOCK):
                    raise
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise TimeoutError(f"timed out acquiring benchmark capture lock in {run}") from exc
                time.sleep(min(_CAPTURE_LOCK_POLL_SECONDS, remaining))
        try:
            yield
        finally:
            release()


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
    parser.add_argument("--glob", metavar="PATTERN", help="Restrict rg searches and source reads to matching file names, e.g. '*.cs'")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()

    modes = [bool(args.cg), bool(args.read), args.rg is not None]
    if sum(modes) != 1:
        parser.error("choose exactly one of --cg, --read, --rg")
    if args.cg and args.glob:
        parser.error("--glob is only enforced for --rg and --read; restrict CodeGraph queries at their indexed source scope")
    root = args.root.resolve()
    if not root.is_dir():
        parser.error(f"missing project root: {root}")
    args.run.mkdir(parents=True, exist_ok=True)
    t0 = time.perf_counter()
    status = "completed"
    failure = None
    timeout_seconds = None

    if args.cg:
        # Preserve the tool's economical default (compact). JSON is opt-in via
        # --arg format=json, rather than a benchmark-only formatting penalty.
        kwargs = {"max_token_budget": 1800}
        for pair in args.arg:
            if "=" not in pair:
                parser.error("--arg requires KEY=VALUE")
            key, raw = pair.split("=", 1)
            kwargs[key] = parse_value(raw)
        cmd = [
            str(args.binary.resolve()), "query", args.cg, "--root", str(root),
            "--args", json.dumps(kwargs, ensure_ascii=False),
        ]
        timeout_seconds = 120
        exit_code, data, error, status, failure = run_subprocess(cmd, root, timeout_seconds)
    elif args.read:
        target = (root / args.read).resolve()
        if not target.is_file() or not target.is_relative_to(root):
            parser.error(f"invalid source path: {args.read}")
        if args.glob and not fnmatch(target.name, args.glob):
            parser.error(f"source path does not match --glob {args.glob}: {args.read}")
        lines = target.read_text(encoding="utf-8-sig", errors="replace").splitlines()
        end = min(len(lines), args.end or len(lines))
        data = ("\n".join(f"{i + 1}| {lines[i]}" for i in range(max(0, args.start - 1), end)) + "\n").encode()
        error = b""
        exit_code = 0
    else:
        cmd = ["rg", "-n", "--no-heading"]
        if args.glob:
            cmd.extend(["-g", args.glob])
        cmd.extend([args.rg, args.path])
        timeout_seconds = 90
        exit_code, data, error, status, failure = run_subprocess(cmd, root, timeout_seconds)
        if exit_code == 1 and status == "nonzero_exit":
            # rg exit 1 means a valid search with no matches, not a subprocess fault.
            status, failure = "no_matches", None

    filename = args.scenario + (".verify.log" if args.verify else ".log")
    path = args.run / filename
    manifest = {
        "utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "scenario": args.scenario,
        "mode": "codegraph" if args.cg else ("read" if args.read else "rg"),
        "tool": args.cg or args.read or args.rg,
        "arguments": dict(
            cg_args=args.arg, root=str(root), verify=args.verify,
            line_range=[args.start, args.end] if args.read else None,
            search_path=args.path if args.rg is not None else None,
            file_glob=args.glob,
        ),
        "exit": exit_code,
        "status": status,
        "failure": failure,
        "timeout_seconds": timeout_seconds,
        "stdout_bytes": len(data),
        "stderr_bytes": len(error),
        "log": filename,
        "duration_ms": round((time.perf_counter() - t0) * 1000),
    }
    with serialized_capture(args.run):
        with path.open("ab") as handle:
            handle.write(data)
            if data and not data.endswith(b"\n"):
                handle.write(b"\n")
            if error:
                handle.write(error)
                if not error.endswith(b"\n"):
                    handle.write(b"\n")
        with (args.run / "operations.jsonl").open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(manifest, ensure_ascii=False) + "\n")
    sys.stdout.buffer.write(data)
    sys.stderr.buffer.write(error)
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
