"""Inject durable CLI outcomes around the real shared ledger; no provider dispatch."""
import copy
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from live_budget import LiveBudget
import run as runner


LIMITS = {"maxProviderRequests": 2, "maxTotalTokens": 1000,
          "maxWallTimeSeconds": 20, "maxCostMicrousd": 5000}
ALLOWANCE = {"providerRequests": 2, "totalTokens": 1000,
             "wallTimeSeconds": 20, "costMicrousd": 5000}
USAGE = {"providerRequests": 1, "inputTokens": 10, "outputTokens": 20,
         "reasoningTokens": 7, "wallTimeSeconds": 1, "costMicrousd": 50,
         "unpricedProviderRequests": 0}
WORKFLOW = {"spec": {"runtime": {"budgets": LIMITS},
                     "providers": {"openai": {"kind": "openai"}},
                     "agents": {"analyst": {"provider": "openai", "model": "fixture-model"}}}}


def inspection(state="succeeded"):
    return {"run": {"runId": "fixture-run", "state": state, "workflow": WORKFLOW},
            "budget": {"usage": copy.deepcopy(USAGE), "reserved": {key: 0 for key in USAGE}},
            "effects": [{"request": {"effectClass": "model", "operation": "openai"},
                         "status": "failed" if state == "failed" else "succeeded"}]}


class RunnerAccountingTests(unittest.TestCase):
    def execute(self, observed, failed=False):
        directory = tempfile.TemporaryDirectory(prefix="agentctl-run-accounting-")
        self.addCleanup(directory.cleanup)
        ledger = LiveBudget(Path(directory.name) / "shared.db", ALLOWANCE)
        self.addCleanup(ledger.close)  # Close before Windows removes the directory.
        args = SimpleNamespace(mode="openai", model="fixture-model", agentctl=Path("fixture-binary"), keep=True)
        case = runner.Case(args, {"id": "01", "directory": "fixture", "openaiWorkflow": "openai.workflow.yaml"})
        self.addCleanup(runner.cleanup_workspace, case.base)

        def setup(workspace):
            workspace.mkdir()
            case.evidence.mkdir()
            runner.save(case.workflow, WORKFLOW)
            for name in ["fixture-tools.json", "example.json", "setup-report.json"]:
                runner.save(workspace / name, {})

        def dispatch(**_):
            self.assertEqual(ledger.summary()["charged"], ALLOWANCE,
                             "the full shared allowance must be reserved before dispatch")
            envelope = {"error": {"runId": "fixture-run"}} if failed else {"data": {"runId": "fixture-run", "state": "succeeded"}}
            case.commands.append({"argv": ["fixture-binary", "run"], "exitCode": 4 if failed else 0,
                                  "envelope": envelope})
            if failed:
                raise AssertionError("injected definitive workflow failure")
            return envelope

        with patch.object(case, "package_workspace", side_effect=setup), \
                patch.object(case, "cli", return_value={}), \
                patch.object(case, "run", side_effect=dispatch), \
                patch.object(case, "inspect", return_value=observed), \
                patch.object(case, "semantics"), patch.object(case, "replay"), \
                patch.object(case, "denial"), patch.object(case, "alternate_cases"):
            result = case.execute(ledger)
        return result, ledger

    def assert_retained(self, result, ledger):
        self.assertEqual(result["status"], "failed")
        self.assertIsNotNone(result["reservationRetained"])
        self.assertEqual(ledger.summary()["charged"], ALLOWANCE)
        row = ledger.connection.execute("SELECT status, charged_json, reserved_json FROM reservations").fetchone()
        self.assertEqual(row[0], "reserved")
        self.assertEqual(json.loads(row[1]), json.loads(row[2]))
        with self.assertRaisesRegex(ValueError, "exhausted"):
            ledger.reserve("would-oversubscribe", {"maxProviderRequests": 1, "maxTotalTokens": 1,
                           "maxWallTimeSeconds": 1, "maxCostMicrousd": 1})

    def test_success_with_any_outstanding_runtime_reservation_keeps_full_shared_charge(self):
        for counter in [*USAGE, "parallelTurns"]:
            with self.subTest(counter=counter):
                observed = inspection()
                observed["budget"]["reserved"][counter] = 1
                self.assert_retained(*self.execute(observed))

    def test_failed_run_with_outstanding_runtime_reservation_keeps_full_shared_charge(self):
        observed = inspection("failed")
        observed["budget"]["reserved"]["inputTokens"] = 400
        self.assert_retained(*self.execute(observed, failed=True))

    def test_incomplete_unpriced_or_uncertain_inspections_cannot_release_allowance(self):
        variants = []
        missing = inspection()
        del missing["budget"]["reserved"]["costMicrousd"]
        variants.append(missing)
        unpriced = inspection()
        unpriced["budget"]["usage"]["unpricedProviderRequests"] = 1
        variants.append(unpriced)
        uncertain = inspection()
        uncertain["effects"][0]["status"] = "uncertain"
        variants.append(uncertain)
        for observed in variants:
            with self.subTest(observed=observed):
                self.assert_retained(*self.execute(observed))

    def test_definitive_success_and_failure_reconcile_actual_usage(self):
        for failed in [False, True]:
            with self.subTest(failed=failed):
                result, ledger = self.execute(inspection("failed" if failed else "succeeded"), failed=failed)
                self.assertEqual(result["status"], "failed" if failed else "passed")
                charged = ledger.summary()["charged"]
                self.assertEqual(charged["providerRequests"], 1)
                self.assertEqual(charged["totalTokens"], 30, "reasoning tokens are already in output tokens")
                self.assertEqual(charged["costMicrousd"], 50)
                self.assertEqual(ledger.summary()["unreconciled"], 0)


if __name__ == "__main__":
    unittest.main()
