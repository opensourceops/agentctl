"""Deterministic subprocess-boundary tests; no provider or external service calls."""
import copy
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("live_command", Path(__file__).with_name("live_command.py"))
wrapper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wrapper)

LIMITS = {"maxProviderRequests": 5, "maxTotalTokens": 1000, "maxWallTimeSeconds": 20, "maxCostMicrousd": 5000}
WORKFLOW = {"spec": {"runtime": {"budgets": LIMITS}, "providers": {"openai": {"kind": "openai"}},
                     "agents": {"review": {"provider": "openai", "model": "fixture-model"}}}}


def envelope(data):
    return json.dumps({"apiVersion": "agentctl.dev/cli/v1", "ok": True, "data": data}).encode()


def inspection(requests=1, inputs=10, outputs=20, cost=50, state="succeeded", status="succeeded"):
    usage = {"providerRequests": requests, "inputTokens": inputs, "outputTokens": outputs,
             "reasoningTokens": 7, "wallTimeSeconds": 2, "costMicrousd": cost, "unpricedProviderRequests": 0}
    return {"run": {"runId": "fixture-run", "workflow": copy.deepcopy(WORKFLOW), "state": state},
            "budget": {"usage": usage, "reserved": {key: 0 for key in usage}},
            "effects": [{"request": {"operation": "openai", "effectClass": "model"}, "status": status}]}


class WrapperTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.budget = Path(self.directory.name) / "shared.sqlite3"
        self.binary = str(Path(self.directory.name) / "agentctl-fixture")
        self.calls, self.output, self.warnings = [], [], []
        self.work = copy.deepcopy(WORKFLOW)
        self.before, self.after = inspection(state="paused"), inspection()
        self.code, self.stdout, self.stderr = 0, envelope({"runId": "fixture-run"}), b"fixture diagnostic\n"
        self.timeout = False
        self.emit = patch.object(wrapper, "emit", side_effect=lambda out=b"", err=b"": self.output.append((out, err)))
        self.warn = patch.object(wrapper, "warning", side_effect=self.warnings.append)
        self.run = patch.object(wrapper.subprocess, "run", side_effect=self.process)
        for patcher in [self.emit, self.warn, self.run]:
            patcher.start()
            self.addCleanup(patcher.stop)

    def rows(self):
        if not self.budget.exists():
            return []
        with closing(sqlite3.connect(self.budget)) as connection:
            return connection.execute("SELECT status, charged_json, reserved_json FROM reservations").fetchall()

    def process(self, args, **options):
        self.calls.append((args, options))
        command = args[1]
        if command == "migrate":
            return subprocess.CompletedProcess(args, 0, envelope({"workflow": self.work}), b"")
        if command == "inspect":
            previous_dispatch = any(call[0][1] in wrapper.DISPATCH for call in self.calls[:-1])
            return subprocess.CompletedProcess(args, 0, envelope(self.after if previous_dispatch else self.before), b"")
        if command in wrapper.DISPATCH and "--plan" not in args and "--check" not in args:
            self.assertEqual(self.rows()[-1][0], "reserved", "allowance must be committed before process dispatch")
            self.assertLessEqual(options["timeout"], json.loads(self.rows()[-1][2])["wallTimeSeconds"])
            if self.timeout:
                raise subprocess.TimeoutExpired(args, options["timeout"], output=b"partial\n", stderr=b"partial error\n")
        return subprocess.CompletedProcess(args, self.code, self.stdout, self.stderr)

    def execute(self, *args):
        return wrapper.execute(self.budget, "fixture-model", [self.binary, *args])

    def test_run_reserves_before_dispatch_reconciles_actual_without_reasoning_duplication(self):
        self.assertEqual(self.execute("run", "workflow.yaml", "--db", "fixture.db", "--output", "json"), 0)
        status, charged, _ = self.rows()[0]
        self.assertEqual(status, "reconciled")
        self.assertEqual(json.loads(charged)["totalTokens"], 30)
        self.assertEqual(self.output, [(self.stdout, self.stderr)])
        self.assertFalse(self.warnings)
        self.assertEqual([call[0][1] for call in self.calls], ["migrate", "run", "inspect"])

    def test_unexpected_output_error_closes_ledger_and_preserves_reservation(self):
        ledger = wrapper.LiveBudget(self.budget)
        with patch.object(wrapper, "LiveBudget", return_value=ledger), patch.object(wrapper, "emit", side_effect=RuntimeError("fixture output failure")):
            with self.assertRaisesRegex(RuntimeError, "fixture output failure"):
                self.execute("run", "workflow.yaml")
        with self.assertRaises(sqlite3.ProgrammingError):
            ledger.connection.execute("SELECT 1")
        status, charged, reserved = self.rows()[0]
        self.assertEqual(status, "reserved")
        self.assertEqual(charged, reserved)
        self.budget.unlink()
        self.assertFalse(self.budget.exists())

    def test_definitive_http_failure_on_stderr_counts_one_request(self):
        self.after = inspection(requests=1, inputs=0, outputs=0, cost=0, state="failed", status="failed")
        self.code, self.stdout = 6, b""
        self.stderr = json.dumps({"ok": False, "error": {"runId": "fixture-run", "exitCode": 6}}).encode()
        self.assertEqual(self.execute("run", "workflow.yaml"), 6)
        self.assertEqual(self.rows()[0][0], "reconciled")
        self.assertEqual(json.loads(self.rows()[0][1])["providerRequests"], 1)

    def test_missing_any_budget_rejects_before_dispatch(self):
        for key in LIMITS:
            with self.subTest(key=key):
                self.work = copy.deepcopy(WORKFLOW)
                del self.work["spec"]["runtime"]["budgets"][key]
                with self.assertRaises(wrapper.BudgetError):
                    self.execute("run", "workflow.yaml")
        self.assertEqual(self.rows(), [])
        self.assertTrue(all(call[0][1] == "migrate" for call in self.calls))

    def test_aggregate_exhaustion_blocks_a_second_process(self):
        wrapper.LiveBudget(self.budget, {"providerRequests": 4, "totalTokens": 2000, "wallTimeSeconds": 100, "costMicrousd": 10000}).connection.close()
        with self.assertRaises(ValueError):
            self.execute("run", "workflow.yaml")
        self.assertEqual(self.rows(), [])

    def test_uncertain_or_started_effect_retains_full_charge(self):
        for status in ["uncertain", "started", "cancelled"]:
            with self.subTest(status=status):
                self.after = inspection(status=status)
                self.assertEqual(self.execute("run", "workflow.yaml"), 2)
                row = self.rows()[-1]
                self.assertEqual(row[0], "reserved")
                self.assertEqual(row[1], row[2])

    def test_outstanding_reserved_or_unpriced_usage_retains_full_charge(self):
        self.after["budget"]["reserved"]["providerRequests"] = 1
        self.execute("run", "workflow.yaml")
        self.assertEqual(self.rows()[-1][0], "reserved")
        self.after = inspection()
        self.after["budget"]["usage"]["unpricedProviderRequests"] = 1
        self.execute("run", "workflow.yaml")
        self.assertEqual(self.rows()[-1][0], "reserved")

    def test_timeout_preserves_partial_output_and_reservation(self):
        self.timeout = True
        self.assertEqual(self.execute("run", "workflow.yaml", "--timeout-seconds", "3"), 124)
        self.assertEqual(self.rows()[0][0], "reserved")
        self.assertEqual(self.output, [(b"partial\n", b"partial error\n")])
        self.assertEqual(self.calls[-1][1]["timeout"], 3)

    def test_resume_reserves_remaining_limits_and_charges_only_delta(self):
        self.after = inspection(requests=2, inputs=17, outputs=31, cost=95)
        self.assertEqual(self.execute("resume", "fixture-run"), 0)
        status, charged, reserved = self.rows()[0]
        self.assertEqual(status, "reconciled")
        self.assertEqual(json.loads(reserved), {"providerRequests": 4, "totalTokens": 970, "wallTimeSeconds": 18, "costMicrousd": 4950})
        self.assertEqual(json.loads(charged)["totalTokens"], 18)
        self.assertEqual(json.loads(charged)["providerRequests"], 1)
        self.assertEqual(json.loads(charged)["costMicrousd"], 45)

    def test_fork_charges_fresh_usage_and_does_not_subtract_source(self):
        self.execute("fork", "source-run")
        self.assertEqual(json.loads(self.rows()[0][1])["totalTokens"], 30)
        self.assertEqual(json.loads(self.rows()[0][2])["totalTokens"], 1000)

    def test_repair_and_retry_load_target_workflow(self):
        for command in ["repair", "retry"]:
            self.execute(command, "target.yaml", "source-run", "--from", "failed")
        self.assertEqual(len(self.rows()), 2)
        self.assertEqual(self.calls[0][0][1:3], ["migrate", "target.yaml"])

    def test_effect_free_commands_are_exact_passthrough(self):
        for args in [("plan", "workflow.yaml"), ("run", "workflow.yaml", "--check"), ("repair", "workflow.yaml", "source", "--plan")]:
            self.assertEqual(self.execute(*args), 0)
            self.assertEqual(self.calls[-1][0], [self.binary, *args])
        self.assertEqual(self.rows(), [])

    def test_missing_run_identity_keeps_reservation(self):
        self.stdout = b"unstructured fixture output\n"
        self.assertEqual(self.execute("run", "workflow.yaml"), 2)
        self.assertEqual(self.rows()[0][0], "reserved")

    def test_actual_overrun_fails_gate_and_keeps_full_actual_charge(self):
        self.after = inspection(requests=6, inputs=900, outputs=200, cost=6000)
        self.assertEqual(self.execute("run", "workflow.yaml"), 2)
        status, charged, _ = self.rows()[0]
        self.assertEqual(status, "exceeded")
        self.assertEqual(json.loads(charged)["providerRequests"], 6)
        self.assertEqual(json.loads(charged)["totalTokens"], 1100)
        self.assertEqual(json.loads(charged)["costMicrousd"], 6000)
        self.assertIn("runtime exceeded", self.warnings[-1])

    def test_accounting_failure_preserves_original_nonzero_exit(self):
        self.after = inspection(status="uncertain")
        self.code = 6
        self.assertEqual(self.execute("run", "workflow.yaml"), 6)
        self.assertEqual(self.rows()[0][0], "reserved")

    def test_running_state_and_signal_keep_reservation(self):
        self.after = inspection(state="running")
        self.execute("run", "workflow.yaml")
        self.assertEqual(self.rows()[-1][0], "reserved")
        self.code = -9
        self.assertEqual(self.execute("run", "workflow.yaml"), 137)
        self.assertEqual(self.rows()[-1][0], "reserved")

    def test_global_and_equals_flags_parse_without_changing_argv(self):
        positional, flags = wrapper.parsed_arguments(["--output=json", "run", "--db=fixture.db", "workflow.yaml", "--var", "name=42"])
        self.assertEqual(positional, ["run", "workflow.yaml"])
        self.assertEqual(flags["--db"], "fixture.db")

    def test_environment_credential_is_redacted_from_both_streams(self):
        with patch.dict(os.environ, {"OPENAI_API_KEY": "fixture-private-key"}):
            self.assertEqual(wrapper.redact(b"prefix fixture-private-key suffix"), b"prefix [REDACTED] suffix")

    def test_uncertain_usage_retains_numeric_audit_metadata_without_payloads(self):
        self.after = inspection(status="uncertain", state="failed")
        self.after["effects"][0]["request"]["input"] = "PRIVATE_TOOL_PAYLOAD"
        self.after["run"]["workflow"]["private"] = "PRIVATE_WORKFLOW_VALUE"
        self.assertEqual(self.execute("run", "workflow.yaml"), 2)
        with closing(sqlite3.connect(self.budget)) as connection:
            encoded = connection.execute("SELECT metadata_json FROM reservations").fetchone()[0]
        self.assertNotIn("PRIVATE_", encoded)
        metadata = json.loads(encoded)
        self.assertEqual(metadata["runId"], "fixture-run")
        self.assertEqual(metadata["observedBudget"]["usage"]["inputTokens"], 10)
        self.assertEqual(metadata["effectStatuses"], ["uncertain"])
        self.assertEqual(self.rows()[0][0], "reserved")


if __name__ == "__main__":
    unittest.main()
