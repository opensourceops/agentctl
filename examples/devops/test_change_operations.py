from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import fixture
import operations


class ChangeOperationsTests(unittest.TestCase):
    def setUp(self):
        owner = tempfile.TemporaryDirectory()
        self.addCleanup(owner.cleanup)
        self.root = Path(owner.name).resolve()
        self.addCleanup(patch.stopall)
        patch.object(fixture, "ROOT", self.root).start()
        (self.root / "fixtures").mkdir()
        self.original = {"timeoutSeconds": 300, "service": "worker", "retries": 3,
                         "security": {"allowPrivilegeEscalation": False}}
        fixture.write("fixtures/configuration.json", self.original)

    def context(self, proposed=30, limit=60):
        return operations.plan_change(fixture, {"configurationPath": "fixtures/configuration.json", "proposedTimeout": proposed, "maxTimeout": limit})

    def test_change_review_accepts_two_inputs_and_preserves_unrelated_fields(self):
        for timeout in [25, 45]:
            context = self.context(timeout)
            review = operations.review_change(fixture, {"context": context, "proposal": {"timeoutSeconds": timeout},
                                                         "review": {"timeoutSeconds": timeout, "approved": True}})
            fixture.write("artifacts/role-timeout.txt", str(timeout))
            payload = {"context": context, "review": review, "requireStagedValue": True}
            first = operations.apply_change(fixture, payload)
            second = operations.apply_change(fixture, payload)
            self.assertEqual(first, second)
            self.assertEqual(first["configuration"], {**self.original, "timeoutSeconds": timeout})
            self.assertEqual(fixture.document("fixtures/configuration.json"), self.original)

    def test_rejected_change_cannot_be_applied_even_with_a_staged_value(self):
        context = self.context(90)
        review = operations.review_change(fixture, {"context": context, "proposal": {"timeoutSeconds": 90},
                                                    "review": {"timeoutSeconds": 90, "approved": False}})
        self.assertFalse(review["approved"])
        fixture.write("artifacts/role-timeout.txt", "90")
        with self.assertRaisesRegex(ValueError, "approval"):
            operations.apply_change(fixture, {"context": context, "review": review, "requireStagedValue": True})
        self.assertFalse((self.root / "artifacts/reviewed-configuration.json").exists())

    def test_malicious_review_and_stale_source_are_rejected(self):
        context = self.context(90)
        with self.assertRaisesRegex(ValueError, "threshold"):
            operations.review_change(fixture, {"context": context, "proposal": {"timeoutSeconds": 90}, "review": {"timeoutSeconds": 90, "approved": True}})
        fixture.write("fixtures/configuration.json", {**self.original, "retries": 9})
        with self.assertRaisesRegex(ValueError, "changed after planning"):
            operations.review_change(fixture, {"context": context, "proposal": {"timeoutSeconds": 90}, "review": {"timeoutSeconds": 90, "approved": False}})

    def test_actual_staging_bytes_must_match_the_reviewed_proposal(self):
        context = self.context()
        review = operations.review_change(fixture, {"context": context, "proposal": {"timeoutSeconds": 30}, "review": {"timeoutSeconds": 30, "approved": True}})
        for token in ["30\n", "90", '{"timeoutSeconds":30}', " 30", "0"]:
            fixture.write("artifacts/role-timeout.txt", token)
            with self.subTest(token=token), self.assertRaises(ValueError):
                operations.apply_change(fixture, {"context": context, "review": review, "requireStagedValue": True})
        self.assertFalse((self.root / "artifacts/reviewed-configuration.json").exists())

    def test_remediation_converges_from_actual_artifacts_across_three_steps(self):
        previous = {"done": False}
        values = []
        for index in range(3):
            context = operations.remediation_context(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": 30, "maxReduction": 120, "iteration": index, "previous": previous})
            fixture.write("artifacts/timeout-seconds.txt", str(context["expectedTimeout"]))
            previous = operations.apply_remediation(fixture, {"context": context, "proposal": {"timeoutSeconds": context["expectedTimeout"]}, "requireStagedValue": True})
            values.append(previous["timeoutSeconds"])
        self.assertEqual(values, [180, 60, 30])
        self.assertTrue(previous["done"])
        final = operations.verify_remediation(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": 30})
        self.assertEqual(final["configuration"], {**self.original, "timeoutSeconds": 30})

    def test_input_derived_repair_and_incomplete_artifact_do_not_trust_done(self):
        context = operations.remediation_context(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": 45, "maxReduction": 50, "iteration": 0, "previous": {"done": False}})
        with self.assertRaisesRegex(ValueError, "input-derived"):
            operations.apply_remediation(fixture, {"context": context, "proposal": {"timeoutSeconds": 45, "done": True}, "requireStagedValue": False})
        result = operations.apply_remediation(fixture, {"context": context, "proposal": {"timeoutSeconds": 250, "done": True}, "requireStagedValue": False})
        self.assertFalse(result["done"])
        with self.assertRaisesRegex(ValueError, "not converged"):
            operations.verify_remediation(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": 45})

    def test_forged_previous_iteration_and_unrelated_changes_fail(self):
        fixture.write("artifacts/remediation.json", {**self.original, "timeoutSeconds": 180, "retries": 99})
        payload = {"configurationPath": "fixtures/configuration.json", "maxTimeout": 30, "maxReduction": 120, "iteration": 1, "previous": {"timeoutSeconds": 180}}
        with self.assertRaisesRegex(ValueError, "unrelated"):
            operations.remediation_context(fixture, payload)
        payload["previous"]["timeoutSeconds"] = 25
        with self.assertRaisesRegex(ValueError, "previous iteration"):
            operations.remediation_context(fixture, payload)


    def test_model_completion_requires_actual_input_derived_staging(self):
        context = operations.prepare_remediation(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": 45, "maxReduction": 120})
        proposal = {"items": [{"state": "succeeded", "output": {"done": True, "timeoutSeconds": 45}}, {"state": "skipped"}]}
        with self.assertRaises(FileNotFoundError):
            operations.validate_remediation_target(fixture, {"context": context, "proposal": proposal})
        fixture.write("artifacts/timeout-seconds.txt", "30")
        with self.assertRaisesRegex(ValueError, "staged timeout"):
            operations.validate_remediation_target(fixture, {"context": context, "proposal": proposal})
        fixture.write("artifacts/timeout-seconds.txt", "45")
        checked = operations.validate_remediation_target(fixture, {"context": context, "proposal": proposal})
        self.assertEqual(checked["targetTimeout"], 45)
        self.assertFalse((self.root / "artifacts/remediation.json").exists())

    def test_repair_step_loop_reports_actual_progress_and_nonconvergence(self):
        for limit, reduction, expected in [(45, 120, [180, 60, 45]), (30, 1, [299, 298, 297])]:
            previous = {"done": False}
            observed = []
            for iteration in range(3):
                previous = operations.remediation_step(fixture, {"configurationPath": "fixtures/configuration.json", "maxTimeout": limit,
                       "maxReduction": reduction, "iteration": iteration, "previous": previous})
                observed.append(previous["timeoutSeconds"])
            self.assertEqual(observed, expected)
            self.assertEqual(previous["done"], expected[-1] <= limit)


if __name__ == "__main__":
    unittest.main()
