import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import fixture
from run import cleanup_workspace


class FixturePortabilityTests(unittest.TestCase):
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
