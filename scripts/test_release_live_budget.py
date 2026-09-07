"""No network/provider calls: original/task/job accounting and failure injection."""
import copy
from concurrent.futures import ThreadPoolExecutor
import importlib.util
import json
from pathlib import Path
import sqlite3
import subprocess
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("release_budget", Path(__file__).with_name("release_live_budget.py"))
budget = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(budget)


class ReleaseBudgetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.original, self.task = self.root / "original.sqlite3", self.root / "task.sqlite3"
        self.execution = self.root / "runner/job.sqlite3"
        self.old_uncertain = {"providerRequests": 12, "totalTokens": 25536, "wallTimeSeconds": 240, "costMicrousd": 2000000}
        with budget.LiveBudget(self.original) as ledger:
            self.old_id = ledger.reserve("prior-task-uncertain", budget.runtime_limits(self.old_uncertain))
        self.context = budget.binding("Ompragash/agentctl-remediation-demo", "a" * 40,
            "Ompragash/agentctl-remediation-demo/.github/workflows/remediate.yml@refs/heads/main", None, "7", "1", "remediate")
        self.environment = {"GITHUB_REPOSITORY": self.context["repository"], "GITHUB_SHA": "a" * 40,
            "GITHUB_WORKFLOW_REF": self.context["workflowRef"], "GITHUB_RUN_ID": "1007", "GITHUB_RUN_NUMBER": "7",
            "GITHUB_RUN_ATTEMPT": "1", "GITHUB_JOB": "remediate", "RUNNER_TEMP": str(self.execution.parent)}
        self.job_limits = {"providerRequests": 12, "totalTokens": 18000, "wallTimeSeconds": 420, "costMicrousd": 900000}
        self.binary = str(self.root / "fixture-agentctl")
        self.workflow = {"spec": {"runtime": {"budgets": budget.runtime_limits(self.job_limits)},
            "providers": {"openai": {"kind": "openai"}},
            "agents": {"analysis": {"model": "gpt-6-astra", "reasoning": {"effort": "high"}},
                       "remediation": {"model": "gpt-6-astra", "reasoning": {"effort": "high"}}}}}

    def initialize(self):
        return budget.initialize(self.original, self.task, "release-fixture")

    def allocate(self, context=None, limits=None):
        self.initialize()
        return budget.lease(self.task, context or self.context, limits or self.job_limits)

    def rows(self, path):
        with budget.LiveBudget(path) as ledger:
            return budget.reservation_rows(ledger)

    def original_row(self, reservation_id):
        return next(row for row in self.rows(self.original) if row["id"] == reservation_id)

    def totals(self, path):
        with budget.LiveBudget(path) as ledger:
            return ledger.summary()["charged"]

    def skipped_evidence(self):
        repository = self.context["repository"]
        run = {"id": 1007, "run_number": 7, "run_attempt": 1, "event": "workflow_dispatch",
               "head_sha": "a" * 40, "head_branch": "main", "path": ".github/workflows/remediate.yml",
               "repository": {"full_name": repository}, "head_repository": {"full_name": repository},
               "status": "completed", "conclusion": "failure"}
        job = {"id": 9001, "run_id": 1007, "run_attempt": 1, "name": "remediate", "head_sha": "a" * 40,
               "head_branch": "main", "run_url": f"https://api.github.com/repos/{repository}/actions/runs/1007",
               "status": "completed", "conclusion": "skipped", "steps": [], "runner_id": None, "runner_name": None}
        return run, [{"total_count": 1, "jobs": [job]}]

    def test_skipped_job_reconciliation_fetches_exact_attempt_and_keeps_original_reserved(self):
        leased = self.allocate()
        run, jobs = self.skipped_evidence()
        with patch.object(budget, "github_json", side_effect=[run, jobs, run, jobs]) as api:
            result = budget.reconcile_skipped(self.task, leased, "1007")
            self.assertEqual(result["status"], "reconciled-skipped")
            self.assertEqual(budget.reconcile_skipped(self.task, leased, "1007")["status"], "already-reconciled")
            self.assertEqual(api.call_count, 4)
            self.assertEqual(api.call_args_list[0].args, ("repos/Ompragash/agentctl-remediation-demo/actions/runs/1007/attempts/1",))
            self.assertTrue(api.call_args_list[1].kwargs["paginate"])
        row = self.rows(self.task)[0]
        self.assertEqual(row["charged"], {key: 0 for key in budget.FIELDS})
        self.assertEqual(row["metadata"]["skippedProofSha256"], budget.digest(row["metadata"]["skippedProof"]))
        self.assertNotIn("receiptSha256", row["metadata"])
        # Equal timestamps sort by ID, not insertion order. Check ties and both orders.
        for old_created, envelope_created in [(0, 0), (0, 1), (1, 0)]:
            with self.subTest(old_created=old_created, envelope_created=envelope_created):
                with budget.LiveBudget(self.original) as ledger:
                    ledger.connection.execute("UPDATE reservations SET created=? WHERE id=?", (old_created, self.old_id))
                    ledger.connection.execute("UPDATE reservations SET created=? WHERE id=?", (envelope_created, leased["envelopeId"]))
                self.assertEqual(self.original_row(leased["envelopeId"])["charged"], budget.TASK_MAXIMUM)
                self.assertEqual(self.original_row(self.old_id)["charged"], self.old_uncertain)
        self.assertEqual(budget.close(self.task)["original"]["charged"], self.old_uncertain)

    def test_skipped_proof_requires_completed_exact_source_repository_workflow_ref_and_attempt(self):
        leased = self.allocate()
        run, jobs = self.skipped_evidence()
        changes = [{"id": 1008}, {"run_number": 8}, {"run_attempt": 2}, {"head_sha": "b" * 40},
                   {"head_branch": "other"}, {"path": ".github/workflows/other.yml"}, {"event": "pull_request"},
                   {"repository": {"full_name": "Other/repo"}}, {"head_repository": {"full_name": "Other/repo"}},
                   {"status": "in_progress"}, {"conclusion": None}]
        for change in changes:
            with self.subTest(change=change), patch.object(budget, "github_json", side_effect=[{**run, **change}, jobs]):
                with self.assertRaisesRegex(ValueError, "run attempt"):
                    budget.reconcile_skipped(self.task, leased, "1007")
                self.assertEqual(self.totals(self.task), self.job_limits)
        direct = copy.deepcopy(leased)
        direct["binding"].update(runId="1008", runNumber=None)
        with patch.object(budget, "github_json") as api, self.assertRaisesRegex(ValueError, "differs from leased"):
            budget.reconcile_skipped(self.task, direct, "1007")
        api.assert_not_called()

    def test_started_failed_missing_ambiguous_or_other_attempt_job_never_releases_lease(self):
        leased = self.allocate()
        run, pages = self.skipped_evidence()
        job = pages[0]["jobs"][0]
        variants = [[{**job, **change}] for change in [
            {"status": "in_progress"}, {"conclusion": "failure"}, {"conclusion": "cancelled"},
            {"steps": [{"name": "Set up job", "conclusion": "skipped"}]}, {"steps": None},
            {"runner_id": 12}, {"runner_name": "assigned"}, {"run_attempt": 2}, {"run_id": 1008},
            {"head_sha": "b" * 40}, {"head_branch": "other"}, {"name": "other-job"},
            {"run_url": "https://api.github.com/repos/Other/repo/actions/runs/1007"}]]
        variants += [[], [job, dict(job, id=9002)]]
        for jobs in variants:
            with self.subTest(jobs=jobs), patch.object(budget, "github_json", side_effect=[run, [{"total_count": len(jobs), "jobs": jobs}]]):
                with self.assertRaises(ValueError):
                    budget.reconcile_skipped(self.task, leased, "1007")
                self.assertEqual(self.totals(self.task), self.job_limits)
                self.assertNotIn("skippedProofSha256", self.rows(self.task)[0]["metadata"])

    def test_missing_pages_wrong_lease_and_unavailable_github_keep_full_reservation(self):
        leased = self.allocate()
        run, pages = self.skipped_evidence()
        bad_pages = [[], {}, [{"total_count": 2, "jobs": pages[0]["jobs"]}], [{"total_count": 0}]]
        for value in bad_pages:
            with patch.object(budget, "github_json", side_effect=[run, value]), self.assertRaises(ValueError):
                budget.reconcile_skipped(self.task, leased, "1007")
        for field in ["taskId", "envelopeId", "leaseId"]:
            other = {**leased, field: "another"}
            with patch.object(budget, "github_json", side_effect=[run, pages]), self.assertRaisesRegex(ValueError, "reserved task lease"):
                budget.reconcile_skipped(self.task, other, "1007")
        with patch.object(budget, "github_json", side_effect=budget.BudgetError("unavailable")), self.assertRaisesRegex(ValueError, "unavailable"):
            budget.reconcile_skipped(self.task, leased, "1007")
        self.assertEqual(self.totals(self.task), self.job_limits)

    def test_skipped_reconciliation_crash_keeps_charges_until_verified_retry(self):
        leased = self.allocate()
        run, pages = self.skipped_evidence()
        with patch.object(budget, "github_json", side_effect=[run, pages]), \
             patch.object(budget.LiveBudget, "reconcile", side_effect=RuntimeError("interrupted")), self.assertRaisesRegex(RuntimeError, "interrupted"):
            budget.reconcile_skipped(self.task, leased, "1007")
        self.assertEqual(self.totals(self.task), self.job_limits)
        self.assertIn("skippedProofSha256", self.rows(self.task)[0]["metadata"])
        with self.assertRaisesRegex(ValueError, "entire original"):
            budget.close(self.task)
        with patch.object(budget, "github_json", side_effect=[run, pages]):
            budget.reconcile_skipped(self.task, leased, "1007")
        self.assertEqual(self.totals(self.task), {key: 0 for key in budget.FIELDS})

    def test_skipped_proof_never_replaces_existing_paid_receipt_or_permits_lease_reuse(self):
        leased = self.allocate()
        self.dispatch(leased)
        budget.reconcile(self.task, budget.receipt(leased, self.execution, self.environment))
        before = self.totals(self.task)
        run, pages = self.skipped_evidence()
        with patch.object(budget, "github_json", side_effect=[run, pages]), self.assertRaisesRegex(ValueError, "different reconciliation"):
            budget.reconcile_skipped(self.task, leased, "1007")
        self.assertEqual(self.totals(self.task), before)
        with self.assertRaisesRegex(ValueError, "cannot be resized or dispatched again"):
            budget.lease(self.task, self.context, self.job_limits)

    def test_github_reader_uses_only_explicit_github_get_and_redacts_failures(self):
        response = subprocess.CompletedProcess([], 0, b'{"id":1007}', b'')
        with patch.object(budget.subprocess, "run", return_value=response) as call:
            self.assertEqual(budget.github_json("repos/Owner/repo/actions/runs/1007"), {"id": 1007})
        self.assertEqual(call.call_args.args[0][:7], ["gh", "api", "--hostname", "github.com", "--method", "GET", "-H"])
        for error in [subprocess.TimeoutExpired([], 60), subprocess.CalledProcessError(1, [], stderr=b'sensitive diagnostic')]:
            with patch.object(budget.subprocess, "run", side_effect=error), self.assertRaisesRegex(ValueError, "retain reservation") as failure:
                budget.github_json("repos/Owner/repo/actions/runs/1007")
            self.assertNotIn("sensitive", str(failure.exception))

    def inspection(self, state="succeeded", status="succeeded", reserved=False, requests=1):
        usage = {"providerRequests": requests, "inputTokens": 10 * requests, "outputTokens": 20 * requests,
            "reasoningTokens": 15 * requests, "wallTimeSeconds": 2 * requests,
            "costMicrousd": 100 * requests, "unpricedProviderRequests": 0}
        counters = {key: 0 for key in usage}
        if reserved:
            counters["providerRequests"] = 1
        return {"run": {"runId": "run-fixture", "state": state, "workflow": self.workflow},
            "budget": {"usage": usage, "reserved": counters},
            "effects": [{"request": {"effectClass": "model", "operation": "openai"}, "status": status}]}

    def dispatch(self, leased, after=None, before=None, timeout=False, command="run"):
        after = after or self.inspection()
        count = 0

        def inspect(_binary, _run, _db):
            return after if count else (before or after)

        def subprocess_run(argv, **options):
            nonlocal count
            count += 1
            self.assertEqual(self.rows(self.execution)[-1]["status"], "reserved")
            self.assertEqual(self.rows(self.task)[-1]["status"], "reserved")
            self.assertEqual(self.original_row(leased["envelopeId"])["status"], "reserved")
            if timeout:
                raise subprocess.TimeoutExpired(argv, options["timeout"])
            return subprocess.CompletedProcess(argv, 0,
                json.dumps({"ok": True, "data": {"runId": "run-fixture"}}).encode(), b"")

        with patch.object(budget.wrapper, "read_data", return_value={"workflow": self.workflow}), \
             patch.object(budget.wrapper, "inspect", side_effect=inspect), \
             patch.object(budget.wrapper.subprocess, "run", side_effect=subprocess_run), \
             patch.object(budget.wrapper, "emit"), patch.object(budget.wrapper, "warning"):
            source = "run-fixture" if command == "resume" else "fixture.yaml"
            result = budget.execute(leased, self.execution, [self.binary, command, source, "--output", "json"], self.environment)
        return result, count

    def test_original_task_allowance_is_reserved_once_and_old_uncertainty_is_untouched(self):
        first = self.initialize()
        self.assertEqual(first, self.initialize())
        self.assertEqual(len(self.rows(self.original)), 2)
        self.assertEqual(self.original_row(self.old_id)["charged"], self.old_uncertain)
        self.assertEqual(self.totals(self.original)["providerRequests"], 42)
        self.assertEqual(first["limits"], budget.TASK_MAXIMUM)
        with self.assertRaisesRegex(ValueError, "different limits or ledger"):
            budget.initialize(self.original, self.root / "another.sqlite3", "release-fixture")

    def test_missing_or_empty_original_never_creates_fresh_independent_allowance(self):
        missing = self.root / "missing.sqlite3"
        with self.assertRaisesRegex(ValueError, "existing ledger"):
            budget.initialize(missing, self.task, "fixture")
        self.assertFalse(missing.exists())
        empty = self.root / "empty.sqlite3"
        empty.touch()
        with self.assertRaisesRegex(ValueError, "original configured"):
            budget.initialize(empty, self.task, "fixture")
        self.assertEqual(empty.stat().st_size, 0)

    def test_original_remaining_allowance_and_task_maximum_fail_before_new_reservation(self):
        with budget.LiveBudget(self.original) as original:
            original.reserve("other-paid-gate", budget.runtime_limits({"providerRequests": 60, "totalTokens": 50000,
                "wallTimeSeconds": 700, "costMicrousd": 1000000}))
        with self.assertRaisesRegex(ValueError, "aggregate live budget exhausted"):
            self.initialize()
        self.assertFalse(self.task.exists())
        with self.assertRaisesRegex(ValueError, "exceeds the task envelope"):
            budget.initialize(self.original, self.task, "fixture", {**budget.TASK_MAXIMUM, "providerRequests": 31})

    def test_concurrent_initialization_does_not_duplicate_original_reservation(self):
        with ThreadPoolExecutor(max_workers=2) as pool:
            results = list(pool.map(lambda _value: self.initialize(), range(2)))
        self.assertEqual(results[0], results[1])
        self.assertEqual(len(self.rows(self.original)), 2)

    def test_initialization_crash_recovers_existing_envelope_without_reset(self):
        with patch.object(budget, "save_config", side_effect=RuntimeError("interrupted")):
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                self.initialize()
        self.assertEqual(len(self.rows(self.original)), 2)
        self.initialize()
        self.assertEqual(len(self.rows(self.original)), 2)

    def test_job_leases_are_idempotent_and_cannot_oversubscribe_task(self):
        leased = self.allocate()
        self.assertEqual(leased, budget.lease(self.task, self.context, self.job_limits))
        with self.assertRaisesRegex(ValueError, "cannot be resized"):
            budget.lease(self.task, self.context, {**self.job_limits, "providerRequests": 11})
        budget.lease(self.task, {**self.context, "runNumber": "8"}, self.job_limits)
        with self.assertRaisesRegex(ValueError, "aggregate live budget exhausted"):
            budget.lease(self.task, {**self.context, "runNumber": "9"}, self.job_limits)
        self.assertEqual(self.totals(self.task)["providerRequests"], 24)

    def test_exact_run_binding_and_context_mismatch_never_create_child_ledger(self):
        leased = self.allocate()
        for field in ["GITHUB_REPOSITORY", "GITHUB_SHA", "GITHUB_WORKFLOW_REF", "GITHUB_RUN_NUMBER", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB"]:
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "does not authorize"):
                budget.claim(leased, self.execution, {**self.environment, field: "wrong"})
            self.assertFalse(self.execution.exists())
        direct = copy.deepcopy(leased)
        direct["binding"].update(runId="1007", runNumber=None)
        budget.verify_ci(direct, self.environment)
        with self.assertRaisesRegex(ValueError, "GITHUB_RUN_ID"):
            budget.verify_ci(direct, {**self.environment, "GITHUB_RUN_ID": "1008"})

    def test_claim_retry_keeps_budget_but_reset_or_other_path_is_rejected(self):
        leased = self.allocate()
        budget.claim(leased, self.execution, self.environment)
        with budget.LiveBudget(self.execution) as child:
            child.reserve("interrupted", budget.runtime_limits(self.job_limits))
        budget.claim(leased, self.execution, self.environment)
        self.assertEqual(self.totals(self.execution), self.job_limits)
        with self.assertRaisesRegex(ValueError, "already claimed"):
            budget.claim(leased, self.execution.with_name("fresh.sqlite3"), self.environment)
        self.execution.unlink()
        with self.assertRaisesRegex(ValueError, "already claimed"):
            budget.claim(leased, self.execution, self.environment)

    def test_wrong_model_or_reasoning_fails_before_dispatch(self):
        leased = self.allocate()
        for change in [{"model": "gpt-5-mini"}, {"reasoning": {"effort": "medium"}}]:
            with self.subTest(change=change):
                original = copy.deepcopy(self.workflow)
                self.workflow["spec"]["agents"]["analysis"].update(change)
                with patch.object(budget.wrapper, "read_data", return_value={"workflow": self.workflow}), \
                     patch.object(budget.wrapper, "execute") as dispatch:
                    with self.assertRaisesRegex(ValueError, "explicit high reasoning"):
                        budget.execute(leased, self.execution, [self.binary, "run", "fixture.yaml"], self.environment)
                    dispatch.assert_not_called()
                self.workflow = original

    def test_success_receipt_reconciles_actual_usage_without_double_counting_reasoning(self):
        leased = self.allocate()
        self.assertEqual(self.dispatch(leased), (0, 1))
        receipt = budget.receipt(leased, self.execution, self.environment)
        self.assertEqual(receipt["charged"]["totalTokens"], 30)
        self.assertEqual(receipt["commands"][0]["observedBudget"]["usage"]["reasoningTokens"], 15)
        budget.reconcile(self.task, receipt)
        self.assertEqual(budget.reconcile(self.task, receipt)["status"], "already-reconciled")
        closed = budget.close(self.task)
        self.assertEqual(closed["taskCharged"]["providerRequests"], 1)
        self.assertEqual(closed["original"]["charged"]["providerRequests"], 13)
        self.assertEqual(closed["original"]["unreconciled"], 1)
        self.assertEqual(self.original_row(self.old_id)["charged"], self.old_uncertain)
        self.assertEqual(budget.close(self.task), closed)
        with self.assertRaisesRegex(ValueError, "closed"):
            budget.lease(self.task, self.context, self.job_limits)

    def test_resume_receipt_checks_only_delta_against_captured_usage_baseline(self):
        leased = self.allocate()
        self.assertEqual(self.dispatch(leased, after=self.inspection(state="paused")), (0, 1))
        self.assertEqual(self.dispatch(leased, after=self.inspection(requests=2), before=self.inspection(state="paused"), command="resume"), (0, 1))
        receipt = budget.receipt(leased, self.execution, self.environment)
        self.assertEqual(receipt["charged"]["providerRequests"], 2)
        self.assertEqual(receipt["charged"]["totalTokens"], 60)
        self.assertEqual(receipt["commands"][1]["usageBefore"]["providerRequests"], 1)
        budget.reconcile(self.task, receipt)

    def test_completed_lease_cannot_dispatch_a_second_run_even_with_remaining_credit(self):
        leased = self.allocate()
        self.assertEqual(self.dispatch(leased), (0, 1))
        with patch.object(budget.wrapper, "read_data", return_value={"workflow": self.workflow}), \
             patch.object(budget.wrapper, "execute") as dispatch:
            with self.assertRaisesRegex(ValueError, "already dispatched"):
                budget.execute(leased, self.execution, [self.binary, "run", "fixture.yaml"], self.environment)
            dispatch.assert_not_called()
        self.assertEqual(self.totals(self.execution)["providerRequests"], 1)

    def test_close_crash_cannot_lease_against_released_original_envelope(self):
        leased = self.allocate()
        self.dispatch(leased)
        budget.reconcile(self.task, budget.receipt(leased, self.execution, self.environment))
        with patch.object(budget, "save_config", side_effect=RuntimeError("interrupted")):
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                budget.close(self.task)
        with self.assertRaisesRegex(ValueError, "no longer reserved"):
            budget.lease(self.task, {**self.context, "runNumber": "8"}, self.job_limits)
        self.assertEqual(budget.close(self.task)["status"], "closed")

    def test_timeout_uncertain_effect_outstanding_tokens_and_unknown_pricing_retain_envelope(self):
        for mode in ["timeout", "effect", "tokens", "price"]:
            with self.subTest(mode=mode):
                context = {**self.context, "runNumber": str(20 + ["timeout", "effect", "tokens", "price"].index(mode))}
                limits = {"providerRequests": 2, "totalTokens": 2000, "wallTimeSeconds": 20, "costMicrousd": 1000}
                leased = self.allocate(context, limits)
                self.environment["GITHUB_RUN_NUMBER"] = context["runNumber"]
                self.execution = self.execution.with_name(mode + ".sqlite3")
                self.workflow["spec"]["runtime"]["budgets"] = budget.runtime_limits(limits)
                after = self.inspection(status="uncertain" if mode == "effect" else "succeeded", reserved=mode == "tokens")
                if mode == "price":
                    after["budget"]["usage"]["unpricedProviderRequests"] = 1
                code, count = self.dispatch(leased, after=after, timeout=mode == "timeout")
                self.assertNotEqual(code, 0)
                self.assertEqual(count, 1)
                with self.assertRaisesRegex(ValueError, "incomplete or exceeded"):
                    budget.receipt(leased, self.execution, self.environment)
                with self.assertRaisesRegex(ValueError, "entire original task envelope retained"):
                    budget.close(self.task)
                self.assertEqual(self.original_row(leased["envelopeId"])["charged"], budget.TASK_MAXIMUM)

    def test_receipt_rejects_clipped_usage_duplicates_and_cross_job_result(self):
        leased = self.allocate()
        self.dispatch(leased)
        original = budget.receipt(leased, self.execution, self.environment)
        clipped = copy.deepcopy(original)
        clipped["commands"][0]["charged"]["totalTokens"] = 1
        clipped["charged"]["totalTokens"] = 1
        with self.assertRaisesRegex(ValueError, "differs from durable"):
            budget.reconcile(self.task, clipped)
        duplicated = copy.deepcopy(original)
        duplicated["commands"] *= 2
        with self.assertRaisesRegex(ValueError, "duplicate"):
            budget.reconcile(self.task, duplicated)
        wrong = copy.deepcopy(original)
        wrong["lease"]["binding"]["sourceSha"] = "b" * 40
        with self.assertRaisesRegex(ValueError, "does not match"):
            budget.reconcile(self.task, wrong)
        self.assertEqual(self.totals(self.task), self.job_limits)

    def test_reconciliation_crash_preserves_binding_and_retries_without_double_charge(self):
        leased = self.allocate()
        self.dispatch(leased)
        receipt = budget.receipt(leased, self.execution, self.environment)
        with patch.object(budget.LiveBudget, "reconcile", side_effect=RuntimeError("interrupted")):
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                budget.reconcile(self.task, receipt)
        self.assertEqual(self.totals(self.task), self.job_limits)
        changed = copy.deepcopy(receipt)
        changed["actualRunId"] = "1008"
        with self.assertRaisesRegex(ValueError, "different receipt"):
            budget.reconcile(self.task, changed)
        budget.reconcile(self.task, receipt)
        self.assertEqual(self.totals(self.task)["providerRequests"], 1)

    def test_receipts_exclude_arbitrary_metadata_and_credentials(self):
        leased = self.allocate()
        self.dispatch(leased)
        with budget.LiveBudget(self.execution) as child:
            row = budget.reservation_rows(child)[0]
            row["metadata"]["secret"] = "fixture-do-not-publish"
            row["metadata"]["prompt"] = "fixture-prompt-do-not-publish"
            child.connection.execute("UPDATE reservations SET metadata_json=? WHERE id=?", (json.dumps(row["metadata"]), row["id"]))
        rendered = json.dumps(budget.receipt(leased, self.execution, self.environment))
        self.assertNotIn("fixture-do-not-publish", rendered)
        self.assertNotIn("fixture-prompt-do-not-publish", rendered)
        self.assertNotIn(str(self.root), rendered)

    def test_tooling_bundle_imports_without_a_framework_checkout_or_python_dependencies(self):
        source = Path(__file__).resolve().parents[1]
        bundle = self.root / "standalone"
        for name in ["scripts/release_live_budget.py", "scripts/live_command.py", "examples/devops/live_budget.py"]:
            destination = bundle / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source / name, destination)
        result = subprocess.run([sys.executable, str(bundle / "scripts/release_live_budget.py"), "execute", "--help"],
            cwd=self.root, env={}, capture_output=True, text=True, timeout=10, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("--execution-ledger", result.stdout)


if __name__ == "__main__":
    unittest.main()
