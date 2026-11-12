"""Regression tests for benchmark output and failure accounting."""

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from benchmarks import query_logger


LOGGER_SCRIPT = Path(query_logger.__file__).resolve()


class QueryLoggerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.run = self.root / "benchmark-logs"

    def invoke(self, *mode_args):
        stdout, stderr = io.BytesIO(), io.BytesIO()
        args = [
            "query_logger.py", "--root", str(self.root), "--run", str(self.run),
            "--scenario", "S1", *mode_args,
        ]
        with patch.object(sys, "argv", args), patch.object(sys, "stdout", SimpleNamespace(buffer=stdout)), patch.object(
            sys, "stderr", SimpleNamespace(buffer=stderr)
        ):
            with self.assertRaises(SystemExit) as stopped:
                query_logger.main()
        entries = [json.loads(line) for line in (self.run / "operations.jsonl").read_text(encoding="utf-8").splitlines()]
        return stopped.exception.code, stdout.getvalue(), stderr.getvalue(), entries[-1], (self.run / "S1.log").read_bytes()

    def test_timeout_preserves_partial_streams_and_manifest(self):
        failure = subprocess.TimeoutExpired(["rg"], 90, output=b"found\n", stderr=b"warning: partial\n")
        with patch.object(query_logger.subprocess, "run", side_effect=failure):
            code, stdout, stderr, entry, logfile = self.invoke("--rg", "test")
        self.assertEqual(code, 124)
        self.assertEqual(stdout, b"found\n")
        self.assertIn(b"warning: partial\n", stderr)
        self.assertIn(b"timed out after 90s", stderr)
        self.assertEqual(logfile, stdout + stderr)
        self.assertEqual(entry["status"], "timeout")
        self.assertEqual(entry["exit"], 124)
        self.assertEqual(entry["stdout_bytes"], len(stdout))
        self.assertEqual(entry["stderr_bytes"], len(stderr))
        self.assertEqual(entry["timeout_seconds"], 90)
        self.assertIn("timed out", entry["failure"])

    def test_failed_spawn_is_recorded_without_losing_diagnostic(self):
        with patch.object(query_logger.subprocess, "run", side_effect=FileNotFoundError("missing executable")):
            code, stdout, stderr, entry, logfile = self.invoke("--cg", "get_file_skeleton")
        self.assertEqual(code, 127)
        self.assertEqual(stdout, b"")
        self.assertIn(b"FileNotFoundError", stderr)
        self.assertIn(b"missing executable", stderr)
        self.assertEqual(logfile, stderr)
        self.assertEqual(entry["status"], "subprocess_error")
        self.assertEqual(entry["timeout_seconds"], 120)
        self.assertEqual(entry["exit"], 127)

    def test_nonzero_process_exit_keeps_raw_stderr(self):
        with patch.object(query_logger.subprocess, "run", return_value=subprocess.CompletedProcess(["rg"], 2, b"\xff", b"rg: bad regex\n")):
            code, stdout, stderr, entry, logfile = self.invoke("--rg", "[")
        self.assertEqual(code, 2)
        self.assertEqual(stdout, b"\xff")
        self.assertEqual(stderr, b"rg: bad regex\n")
        self.assertEqual(logfile, b"\xff\nrg: bad regex\n")
        self.assertEqual(entry["status"], "nonzero_exit")
        self.assertIn("status 2", entry["failure"])

    def test_rg_no_matches_is_not_mislabeled_as_execution_failure(self):
        with patch.object(query_logger.subprocess, "run", return_value=subprocess.CompletedProcess(["rg"], 1, b"", b"")):
            code, stdout, stderr, entry, logfile = self.invoke("--rg", "missing")
        self.assertEqual(code, 1)
        self.assertEqual((stdout, stderr, logfile), (b"", b"", b""))
        self.assertEqual(entry["status"], "no_matches")
        self.assertIsNone(entry["failure"])

    def test_glob_limits_rg_search_to_cs_and_records_scope(self):
        (self.root / "scene.prefab").write_text("TargetMethod\n", encoding="utf-8")
        (self.root / "component.cs").write_text("TargetMethod();\n", encoding="utf-8")
        code, stdout, stderr, entry, logfile = self.invoke(
            "--rg", "TargetMethod", "--path", ".", "--glob", "*.cs"
        )
        self.assertEqual(code, 0)
        self.assertIn(b"component.cs", stdout)
        self.assertNotIn(b"scene.prefab", stdout)
        self.assertEqual(entry["arguments"]["file_glob"], "*.cs")
        self.assertEqual(stderr, b"")
        self.assertEqual(logfile, stdout)

    def test_glob_on_read_rejects_other_file_types(self):
        (self.root / "scene.prefab").write_text("TargetMethod\n", encoding="utf-8")
        (self.root / "component.cs").write_text("TargetMethod();\n", encoding="utf-8")
        code, stdout, _, entry, _ = self.invoke("--read", "component.cs", "--glob", "*.cs")
        self.assertEqual(code, 0)
        self.assertIn(b"TargetMethod", stdout)
        self.assertEqual(entry["arguments"]["file_glob"], "*.cs")
        rejected = subprocess.run(
            [
                sys.executable, str(LOGGER_SCRIPT), "--root", str(self.root),
                "--run", str(self.run), "--scenario", "S2", "--read", "scene.prefab",
                "--glob", "*.cs",
            ],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.assertEqual(rejected.returncode, 2)
        self.assertIn(b"does not match --glob", rejected.stderr)
        self.assertFalse((self.run / "S2.log").exists())

    def test_subprocess_internal_error_preserves_partial_output(self):
        exc = subprocess.SubprocessError("wait failed")
        exc.stdout = b"partial stdout"
        exc.stderr = b"partial stderr"
        with patch.object(query_logger.subprocess, "run", side_effect=exc):
            code, stdout, stderr, entry, logfile = self.invoke("--cg", "get_file_skeleton")
        self.assertEqual(code, 125)
        self.assertEqual(stdout, b"partial stdout")
        self.assertTrue(stderr.startswith(b"partial stderr\n[query_logger] subprocess failed"))
        self.assertEqual(logfile, stdout + b"\n" + stderr)
        self.assertEqual(entry["status"], "subprocess_error")

    def test_read_mode_maintains_original_manifest_shape(self):
        (self.root / "sample.cs").write_text("class Sample {}\n", encoding="utf-8")
        code, stdout, stderr, entry, logfile = self.invoke("--read", "sample.cs", "--start", "1", "--end", "1")
        self.assertEqual(code, 0)
        self.assertEqual(stdout, b"1| class Sample {}\n")
        self.assertEqual(logfile, stdout)
        self.assertEqual(stderr, b"")
        self.assertEqual(entry["mode"], "read")
        self.assertEqual(entry["arguments"]["line_range"], [1, 1])
        self.assertIsNone(entry["timeout_seconds"])

    def test_codegraph_defaults_to_compact_and_json_is_explicit(self):
        commands = []

        def record(command, **kwargs):
            commands.append(command)
            return subprocess.CompletedProcess(command, 0, b"ok\n", b"")

        with patch.object(query_logger.subprocess, "run", side_effect=record):
            self.invoke("--cg", "search_universal", "--arg", "query=Target")
            self.invoke("--cg", "search_universal", "--arg", "query=Target", "--arg", "format=json")
        implicit = json.loads(commands[0][-1])
        explicit = json.loads(commands[1][-1])
        self.assertNotIn("format", implicit)
        self.assertEqual(explicit["format"], "json")

    def test_concurrent_processes_keep_every_manifest_entry_and_log_byte(self):
        """Many CLI instances append to the same run, scenario and verify logs."""
        samples = 64
        jobs = []
        for i in range(samples):
            scenario = f"S{i % 4 + 1}"
            verify = i % 3 == 0
            args = [
                sys.executable, str(LOGGER_SCRIPT), "--root", str(self.root),
                "--run", str(self.run), "--scenario", scenario,
            ]
            if i % 5 == 0:
                # No CodeGraph binary is launched. These deliberately missing
                # executables exercise concurrent stderr and error manifests.
                marker = f"marker={i:03d}"
                name = marker
                args += ["--cg", "not_a_real_tool", "--arg", marker, "--binary", str(self.root / f"missing-{i:03d}.exe")]
            else:
                source = self.root / f"case-{i:03d}.cs"
                # Different sizes expose interleaving; each operation is unique.
                source.write_text(f"// unique-{i:03d}\n" + (f"class Case{i} {{}}\n" * (1 + i % 7)), encoding="utf-8")
                name = source.name
                args += ["--read", source.name]
            if verify:
                args.append("--verify")
            jobs.append((name, scenario + (".verify.log" if verify else ".log"), args))

        # Thread workers each spawn a *separate Python interpreter*, exercising
        # OS file locks instead of serializing within this unittest process.
        def invoke(job):
            name, log, args = job
            result = subprocess.run(args, capture_output=True, timeout=40, check=False)
            return name, log, result

        with ThreadPoolExecutor(max_workers=16) as pool:
            results = list(pool.map(invoke, jobs))

        by_name = {}
        for name, log, proc in results:
            if name.startswith("marker="):
                self.assertEqual(proc.returncode, 127, f"{name}: {proc.stderr!r}")
                self.assertIn(b"FileNotFoundError", proc.stderr)
            else:
                self.assertEqual(proc.returncode, 0, f"{name}: {proc.stderr!r}")
                self.assertEqual(proc.stderr, b"", name)
            by_name[name] = (log, proc.stdout, proc.stderr)

        entries = [json.loads(line) for line in (self.run / "operations.jsonl").read_text(encoding="utf-8").splitlines()]
        self.assertEqual(len(entries), samples, "lost or duplicate manifest entries")
        def entry_name(entry):
            return entry["arguments"]["cg_args"][0] if entry["mode"] == "codegraph" else entry["tool"]

        self.assertEqual(len({entry_name(entry) for entry in entries}), samples, "duplicate/missing operations")
        self.assertEqual(set(by_name), {entry_name(entry) for entry in entries})

        expected_logs = {}
        for entry in entries:
            name = entry_name(entry)
            log, stdout, stderr = by_name[name]
            self.assertEqual(entry["log"], log)
            if name.startswith("marker="):
                self.assertEqual(entry["mode"], "codegraph")
                self.assertEqual(entry["status"], "subprocess_error")
                self.assertEqual(entry["exit"], 127)
            else:
                self.assertEqual(entry["mode"], "read")
                self.assertEqual(entry["status"], "completed")
                self.assertEqual(entry["exit"], 0)
            self.assertEqual(entry["stdout_bytes"], len(stdout))
            self.assertEqual(entry["stderr_bytes"], len(stderr))
            expected = stdout + (b"\n" if stdout and not stdout.endswith(b"\n") else b"")
            expected += stderr + (b"\n" if stderr and not stderr.endswith(b"\n") else b"")
            expected_logs.setdefault(log, bytearray()).extend(expected)

        self.assertGreater(len(expected_logs), 1)
        for log, expected in expected_logs.items():
            self.assertEqual((self.run / log).read_bytes(), bytes(expected), f"out-of-order/interleaved log: {log}")

    def test_capture_lock_has_bounded_wait_and_remains_usable(self):
        self.run.mkdir(parents=True)
        with query_logger.serialized_capture(self.run):
            with patch.object(query_logger, "_CAPTURE_LOCK_TIMEOUT_SECONDS", 0.1), patch.object(
                query_logger, "_CAPTURE_LOCK_POLL_SECONDS", 0.005
            ):
                with self.assertRaises(TimeoutError):
                    with query_logger.serialized_capture(self.run):
                        self.fail("second independent file handle acquired the same lock")
        # An exception while contending must leave the original lock usable.
        with query_logger.serialized_capture(self.run):
            pass

    @unittest.skipUnless(os.name == "posix", "requires real fcntl/flock on POSIX")
    def test_posix_file_lock_is_released_on_process_exit(self):
        """A real POSIX child waits for the parent's flock, then enters."""
        import fcntl

        self.run.mkdir(parents=True)
        script = (
            "import sys\n"
            "from pathlib import Path\n"
            "from benchmarks.query_logger import serialized_capture\n"
            "print('ready', flush=True)\n"
            "with serialized_capture(Path(sys.argv[1])):\n"
            "    print('acquired', flush=True)\n"
        )
        with query_logger.serialized_capture(self.run):
            child = subprocess.Popen(
                [sys.executable, "-c", script, str(self.run)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
            self.assertEqual(child.stdout.readline(), b"ready\n")
            # The parent owns the lock while this check runs.
            with (self.run / ".capture.lock").open("rb") as other:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(other.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        stdout, stderr = child.communicate(timeout=10)
        self.assertEqual(child.returncode, 0, stderr)
        self.assertEqual(stdout.strip(), b"acquired")


if __name__ == "__main__":
    unittest.main()
