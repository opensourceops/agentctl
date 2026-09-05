import json
import hashlib
from contextlib import closing
import os
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import fixture
from run import cleanup_workspace, latest_run_id


class FixturePortabilityTests(unittest.TestCase):
    def test_remediation_token_is_byte_exact_before_serialization(self):
        for token in [b"30", b"30\n", b" 30", b"31", b'{"timeoutSeconds":30}\n']:
            with self.subTest(token=token), tempfile.TemporaryDirectory() as directory, patch.object(fixture, "ROOT", Path(directory).resolve()):
                root = Path(directory)
                (root / "artifacts").mkdir()
                (root / "artifacts/timeout-seconds.txt").write_bytes(token)
                final = root / "artifacts/remediation.json"
                if token == b"30":
                    report = fixture.analyze("20", {})
                    self.assertTrue(report["validated"])
                    self.assertEqual(report["configuration"], {"timeoutSeconds": 30})
                    self.assertEqual(final.read_bytes(), b'{"timeoutSeconds":30}\n')
                else:
                    with self.assertRaisesRegex(ValueError, "approved decimal bytes"):
                        fixture.analyze("20", {})
                    self.assertFalse(final.exists())

    def test_pipeline_patch_preserves_proposal_bytes_with_host_autocrlf_enabled(self):
        before = json.dumps({"permissions": "write-all", "jobs": {"test": {"timeout-minutes": 90}}}, indent=2) + "\n"
        expected = json.dumps({"permissions": "read-all", "jobs": {"test": {"timeout-minutes": 15}}}, indent=2) + "\n"
        git_defaults = {"GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.autocrlf", "GIT_CONFIG_VALUE_0": "true"}
        with tempfile.TemporaryDirectory() as directory, patch.object(fixture, "ROOT", Path(directory).resolve()), patch.dict(os.environ, git_defaults):
            root = Path(directory)
            (root / "fixtures").mkdir()
            (root / "fixtures/pipeline.json").write_bytes(before.replace("\n", "\r\n").encode())
            report = fixture.analyze("03", {})
            self.assertTrue(report["validated"])
            self.assertEqual(len(report["violations"]), 2)
            self.assertTrue(report["patch"]["appliedAndVerified"])
            self.assertEqual((root / "artifacts/patch-workspace/pipeline.json").read_bytes(), expected.encode())

    def test_explicit_git_works_with_empty_environment_and_crlf_patch(self):
        executable = shutil.which("git")
        self.assertIsNotNone(executable, "Git is a fixture prerequisite")
        executable = Path(executable).resolve()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shutil.copy2(fixture.__file__, root / "fixture.py")
            support = {"git": {"executable": str(executable),
                               "sha256": hashlib.sha256(executable.read_bytes()).hexdigest()}}
            (root / "fixture-tools.json").write_text(json.dumps(support))
            script = (
                f"import sys; sys.path.insert(0, {str(root)!r}); import fixture; "
                "fixture.patch('sample.txt', 'before\\r\\n', 'after\\r\\n'); "
                "assert fixture.path('artifacts/patch-workspace/sample.txt').read_bytes() == b'after\\r\\n'"
            )
            child = subprocess.run(["python3", "-c", script], executable=sys.executable,
                                   cwd=root, env={}, capture_output=True, text=True, timeout=10)
            self.assertEqual(child.returncode, 0, child.stderr)
            support["git"]["sha256"] = "0" * 64
            (root / "fixture-tools.json").write_text(json.dumps(support))
            rejected = subprocess.run(["python3", "-c", script], executable=sys.executable,
                                      cwd=root, env={}, capture_output=True, text=True, timeout=10)
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("Git installation changed after runner preflight", rejected.stderr)

    def test_package_digest_rejects_changed_line_endings(self):
        source = Path(__file__).resolve().parent / "10-release-readiness/fixtures"
        package = (source / "package.txt").read_bytes()
        gates = json.loads((source / "gates.json").read_text())
        self.assertEqual(hashlib.sha256(package).hexdigest(), gates["packageSha256"])
        with tempfile.TemporaryDirectory() as directory, patch.object(fixture, "ROOT", Path(directory).resolve()):
            root = Path(directory)
            shutil.copytree(source, root / "fixtures")
            (root / "fixtures/package.txt").write_bytes(package.replace(b"\n", b"\r\n"))
            self.assertEqual(fixture.analyze("10", {})["blocking"], ["security-review", "package-digest"])

    def test_run_probe_closes_sqlite_handle_before_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "runtime.db"
            with closing(sqlite3.connect(database)) as connection:
                connection.execute("CREATE TABLE runs (run_id TEXT, created_at INTEGER)")
                connection.execute("INSERT INTO runs VALUES ('fixture-run', 1)")
                connection.commit()
            connection = sqlite3.connect(database)
            with patch("run.sqlite3.connect", return_value=connection):
                self.assertEqual(latest_run_id(database), "fixture-run")
            with self.assertRaises(sqlite3.ProgrammingError):
                connection.execute("SELECT 1")
            database.unlink()

    def test_nested_python_runs_with_empty_environment(self):
        script = (
            f"import sys; sys.path.insert(0, {str(Path(__file__).resolve().parent)!r}); "
            "import fixture; result = fixture.command([fixture.python_executable(), "
            "'-c', 'print(42)']); print(result.stdout.strip())"
        )
        child = subprocess.run(["python3", "-c", script], executable=sys.executable,
                               env={}, capture_output=True, text=True, timeout=10)
        self.assertEqual(child.returncode, 0, child.stderr)
        self.assertEqual(child.stdout.strip(), "42")

    def test_cleanup_removes_readonly_artifact(self):
        root = Path(tempfile.mkdtemp(prefix="agentctl-devops-cleanup-"))
        blob = root / "blob"
        blob.write_bytes(b"immutable fixture")
        blob.chmod(stat.S_IREAD)
        cleanup_workspace(root)
        self.assertFalse(root.exists())

    def test_cleanup_retries_readonly_permission_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            blob = root / "blob"
            blob.write_text("fixture")
            blob.chmod(stat.S_IREAD)
            def failed_remove(base, onerror):
                def retry(filename):
                    self.assertTrue(Path(filename).stat().st_mode & stat.S_IWRITE)
                    os.unlink(filename)
                error = PermissionError("read-only artifact")
                onerror(retry, str(blob), (PermissionError, error, None))
            with patch("run.shutil.rmtree", side_effect=failed_remove):
                cleanup_workspace(root)
            self.assertFalse(blob.exists())

    def test_cleanup_rejects_permission_repair_outside_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "owned"
            root.mkdir()
            outside = Path(directory) / "outside"
            outside.write_text("retain")
            def failed_remove(base, onerror):
                error = PermissionError("external path")
                onerror(os.unlink, str(outside), (PermissionError, error, None))
            with patch("run.shutil.rmtree", side_effect=failed_remove):
                with self.assertRaises(PermissionError):
                    cleanup_workspace(root)
            self.assertEqual(outside.read_text(), "retain")

    def test_failure_traceback_omits_exception_values(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(fixture, "ROOT", Path(directory).resolve()):
            try:
                raise ValueError("SENSITIVE_FIXTURE_VALUE")
            except ValueError as error:
                fixture.record_failure(error)
            contents = (Path(directory) / "evidence/fixture-error.json").read_text()
            self.assertNotIn("SENSITIVE_FIXTURE_VALUE", contents)
            diagnostic = json.loads(contents)
            self.assertEqual(diagnostic["errorType"], "ValueError")
            self.assertTrue(diagnostic["frames"])
            self.assertTrue(diagnostic["exceptionValuesRedacted"])


if __name__ == "__main__":
    unittest.main()
