"""Acceptance-runner evidence survives prerequisite failures and detects real mutations."""
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

from run import Case, artifact_digests, cleanup_workspace


class RunnerContractsTests(unittest.TestCase):
    def test_denial_snapshot_ignores_only_the_engine_root_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            before = artifact_digests(root)
            (artifacts / ".lock").write_bytes(b"")
            self.assertEqual(before, artifact_digests(root))
            (artifacts / "nested").mkdir()
            (artifacts / "nested/.lock").write_bytes(b"user artifact")
            self.assertIn("artifacts/nested/.lock", artifact_digests(root))
            (artifacts / "service.json").write_text('{"healthy":true}')
            observed = artifact_digests(root)
            (artifacts / "service.json").write_text('{"healthy":false}')
            self.assertNotEqual(observed, artifact_digests(root))

    def test_package_failure_is_recorded_before_any_live_reservation(self):
        args = SimpleNamespace(mode="openai", model="gpt-5-mini", keep=True, agentctl=Path("unused"))
        entry = {"id": "01", "directory": "01-ci-diagnosis", "openaiWorkflow": "openai.workflow.yaml"}
        case = Case(args, entry)
        ledger = mock.Mock()
        try:
            failed = subprocess.CompletedProcess([], 1, "", "declared prerequisite is unavailable")
            with mock.patch("run.subprocess.run", return_value=failed) as command:
                result = case.execute(ledger)
            self.assertEqual(result["status"], "failed")
            self.assertIn("declared prerequisite is unavailable", result["error"])
            self.assertTrue(result["workspaceRetained"])
            self.assertEqual(result["commands"][0]["exitCode"], 1)
            self.assertEqual(command.call_count, 1)
            ledger.reserve.assert_not_called()
            retained = json.loads((case.evidence / "result.json").read_text())
            self.assertEqual(retained["status"], "failed")
            self.assertIsNone(retained["reservationRetained"])
        finally:
            cleanup_workspace(case.base)


if __name__ == "__main__":
    unittest.main()
