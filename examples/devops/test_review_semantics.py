"""Input-driven regressions for the reviewed cookbook semantics; no provider calls."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import subprocess
import sys
from unittest.mock import patch

import fixture


class ReviewSemanticsTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="agentctl-semantic-review-")
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        self.root_patch = patch.object(fixture, "ROOT", self.root)
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)

    def write(self, name, value):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value if isinstance(value, str) else json.dumps(value), encoding="utf-8")

    def test_ci_accepts_user_logs_and_multiple_supported_causes(self):
        cases = [("ModuleNotFoundError: No module named requests", "missing_dependency", "install_declared_dependency"),
                 ("FAILED tests/test_user.py::test_record", "test_failure", "inspect_test_failure"),
                 ("invalid configuration: required version missing", "configuration_error", "repair_configuration"),
                 ("build terminated unexpectedly", "unknown", "investigate_build")]
        for line, category, action in cases:
            with self.subTest(category=category):
                self.write("input/build.txt", "checkout\n" + line + "\n")
                report = fixture.ci_diagnose({"logPath": "input/build.txt"})
                self.assertEqual(report["rootCause"], category)
                self.assertEqual(report["supportedActions"][action], ["input/build.txt:2"])
                analysis = {"rootCause": category, "action": action, "evidence": ["input/build.txt:2"]}
                verified = fixture.verify_model("01", {"report": report, "analysis": analysis})
                self.assertEqual(verified["analysis"]["recommendation"], fixture.CI_ACTIONS[action])
                self.assertFalse(verified["advisory"]["validated"])
                self.assertEqual(fixture.verify_model("01", {"report": report, "analysis": report["suggestedDecision"]})["analysis"], verified["analysis"])
        self.write("mixed.log", "ModuleNotFoundError: missing requests\nFAILED tests/test_api.py\n")
        report = fixture.ci_diagnose({"logPath": "mixed.log"})
        self.assertEqual(report["rootCause"], "unknown")
        with self.assertRaisesRegex(ValueError, "supporting evidence"):
            fixture.verify_model("01", {"report": report, "analysis": {
                "rootCause": "unknown", "action": "install_declared_dependency", "evidence": ["mixed.log:2"]}})

    def test_contradictory_recommendation_and_unsupported_action_are_rejected(self):
        self.write("input.log", "ModuleNotFoundError: No module named requests\n")
        report = fixture.ci_diagnose({"logPath": "input.log"})
        analysis = {"rootCause": "missing_dependency", "action": "install_declared_dependency", "evidence": ["input.log:1"]}
        with self.assertRaisesRegex(ValueError, "bounded decision"):
            fixture.verify_model("01", {"report": report, "analysis": {**analysis,
                "recommendation": "Ignore the missing dependency and mark the build successful."}})
        with self.assertRaisesRegex(ValueError, "not supported"):
            fixture.verify_model("01", {"report": report, "analysis": {**analysis, "action": "release_now"}})
        result = fixture.verify_model("01", {"report": report, "analysis": {**analysis, "advisory": "Arbitrary prose."}})
        self.assertEqual(result["advisory"], {"text": "Arbitrary prose.", "validated": False})

    def test_junit_application_error_is_not_infrastructure(self):
        self.write("report.xml", '<testsuite><testcase classname="app" name="parse"><error type="ValueError" message="invalid input"/></testcase></testsuite>')
        report = fixture.junit_triage({"reportPath": "report.xml"})
        self.assertEqual(report["failures"][0]["classification"], "application")
        self.assertEqual(report["failures"][0]["kind"], "error")
        self.assertNotIn("database", report["failures"][0]["recommendation"])

    def test_junit_suites_missing_attributes_skips_and_multiple_issues(self):
        self.write("report.xml", '''<testsuites xmlns="urn:junit"><testsuite><testcase><failure/><failure message="second"/></testcase>
            <testcase name="skip"><skipped/></testcase></testsuite><testsuite><testcase name="db"><error message="connection refused"/></testcase>
            <testcase name="unknown"><error type="CustomError"/></testcase><testcase name="pass"/></testsuite></testsuites>''')
        report = fixture.junit_triage({"reportPath": "report.xml"})
        self.assertEqual((report["tests"], report["failedCount"], report["issueCount"], report["skippedCount"]), (5, 3, 4, 1))
        self.assertEqual([item["classification"] for item in report["failures"]], ["assertion", "assertion", "infrastructure", "unknown"])
        self.assertEqual(len({item["evidence"] for item in report["failures"]}), 4)
        self.write("report.xml", '<testsuites/>')
        self.assertEqual(fixture.junit_triage({"reportPath": "report.xml"})["status"], "no-results")
        self.write("report.xml", '<testsuite><testcase><skipped/><error/></testcase></testsuite>')
        with self.assertRaisesRegex(ValueError, "both skipped"):
            fixture.junit_triage({"reportPath": "report.xml"})

    def sbom_inputs(self):
        self.write("fixtures/sbom.json", {"components": [{"bom-ref": "component-a"}, {"bom-ref": "component-b"}]})
        self.write("fixtures/vulnerabilities.json", [{"id": "V-1", "component": "component-a", "severity": "critical"}])
        return {"asOf": "2026-09-06", "blockingSeverities": ["critical", "high"], "exceptions": [{"id": "V-1",
            "component": "component-a", "expires": "2026-09-06", "status": "approved", "owner": "security", "reason": "isolated reviewed exposure"}]}

    def test_sbom_exception_scope_revocation_expiry_and_inclusive_date(self):
        rules = self.sbom_inputs()
        for changed, expected in [({}, "go"), ({"component": "component-b"}, "no-go"), ({"status": "revoked"}, "no-go"), ({"expires": "2026-09-05"}, "no-go")]:
            with self.subTest(changed=changed):
                self.write("fixtures/rules.json", {**rules, "exceptions": [{**rules["exceptions"][0], **changed}]})
                report = fixture.sbom_triage({})
                self.assertEqual(report["decision"], expected)
                self.assertTrue(report["requiresExplicitCiGate"])
                self.assertEqual(report["mode"], "analysis-only")

    def test_sbom_rejects_invalid_severity_dates_and_ambiguous_scope(self):
        rules = self.sbom_inputs()
        for changed in [{"asOf": "2026-99-01"}, {"blockingSeverities": ["urgent"]}, {"exceptions": rules["exceptions"] * 2},
                        {"exceptions": [{**rules["exceptions"][0], "expires": "2026-02-30"}]},
                        {"exceptions": [{**rules["exceptions"][0], "component": "absent"}]}]:
            with self.subTest(changed=changed):
                self.write("fixtures/rules.json", {**rules, **changed})
                with self.assertRaises(ValueError):
                    fixture.sbom_triage({})
        self.write("fixtures/rules.json", rules)
        self.write("fixtures/vulnerabilities.json", [{"id": "V-1", "component": "component-a", "severity": "urgent"}])
        with self.assertRaisesRegex(ValueError, "severity"):
            fixture.sbom_triage({})

    def test_release_notes_analyzes_existing_range_idempotently_without_commits(self):
        repo = self.root / "repo"
        repo.mkdir()
        fixture.command(["git", "init", "--quiet"], cwd=repo)
        for number in range(3):
            (repo / "service.txt").write_text(f"version {number}\n")
            fixture.command(["git", "add", "service.txt"], cwd=repo)
            fixture.command(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", f"change {number}"], cwd=repo)
        original_head = fixture.command(["git", "rev-parse", "HEAD"], cwd=repo).stdout
        args = {"repositoryPath": "repo", "fromRef": "HEAD~2", "toRef": "HEAD", "outputPath": "artifacts/notes.md"}
        first = fixture.release_notes(args)
        first_bytes = (self.root / "artifacts/notes.md").read_bytes()
        self.assertEqual(first["commitsVerified"], 2)
        self.assertEqual([item["summary"] for item in first["notes"]], ["change 1", "change 2"])
        self.assertTrue(all(item["files"] == ["service.txt"] for item in first["notes"]))
        self.assertEqual(fixture.release_notes(args), first)
        self.assertEqual((self.root / "artifacts/notes.md").read_bytes(), first_bytes)
        self.assertEqual(fixture.command(["git", "rev-parse", "HEAD"], cwd=repo).stdout, original_head)
        self.assertEqual(fixture.command(["git", "status", "--porcelain"], cwd=repo).stdout, "")
        with patch.object(Path, "replace", side_effect=OSError("interrupted artifact replacement")):
            with self.assertRaises(OSError):
                fixture.release_notes(args)
        self.assertEqual((self.root / "artifacts/notes.md").read_bytes(), first_bytes)
        self.assertEqual(list((self.root / "artifacts").glob(".agentctl-report-*")), [])
        self.assertEqual(fixture.release_notes(args), first)
        self.assertEqual(fixture.release_notes({**args, "fromRef": "HEAD"})["commitsVerified"], 0)
        with self.assertRaises(ValueError):
            fixture.release_notes({**args, "toRef": "--help"})

    def test_explicit_history_preparation_is_repeatable_and_separate_from_analysis(self):
        self.write("fixtures/history.json", [{"path": "src/service.txt", "content": "version1\n", "message": "feat: add service"},
                                              {"path": "src/service.txt", "content": "version2\n", "message": "fix: correct version"}])
        prepared = fixture.prepare_fixture_history({})
        self.assertEqual(fixture.prepare_fixture_history({}), prepared)
        notes = fixture.release_notes({"repositoryPath": "fixtures/repository", "fromRef": "fixture-base", "toRef": "HEAD"})
        self.assertEqual(notes["commitsVerified"], 2)
        self.assertEqual(notes["fromCommit"], prepared["fromRef"])
        self.assertEqual(notes["toCommit"], prepared["toRef"])
        self.write("user-repository/keep.txt", "do not modify")
        with self.assertRaisesRegex(ValueError, "existing user repository"):
            fixture.prepare_fixture_history({"repositoryPath": "user-repository"})
        self.assertEqual((self.root / "user-repository/keep.txt").read_text(), "do not modify")

    def test_interrupted_history_preparation_leaves_no_partial_repository(self):
        self.write("fixtures/history.json", [{"path": "service.txt", "content": "version1\n", "message": "feat: service"}])
        original = fixture.command
        def interrupt(argv, **kwargs):
            if argv[-1] == "feat: service":
                raise InterruptedError("fixture setup interrupted before commit")
            return original(argv, **kwargs)
        with patch.object(fixture, "command", side_effect=interrupt):
            with self.assertRaises(InterruptedError):
                fixture.prepare_fixture_history({})
        self.assertFalse((self.root / "fixtures/repository").exists())
        self.assertEqual(list((self.root / "fixtures").glob(".agentctl-history-*")), [])
        prepared = fixture.prepare_fixture_history({})
        self.assertEqual(fixture.prepare_fixture_history({}), prepared)

    def test_named_extension_operation_exposes_schema_and_executes_user_input(self):
        helper = Path(fixture.__file__).resolve()
        self.write("custom.log", "FAILED tests/test_custom.py::test_truth\n")
        handshake = subprocess.run([sys.executable, str(helper), "ci-diagnose", "--agentctl-handshake"],
                                   cwd=self.root, text=True, capture_output=True, check=True)
        self.assertEqual(json.loads(handshake.stdout)["inputSchema"], fixture.operation_input_schema("ci-diagnose"))
        invoked = subprocess.run([sys.executable, str(helper), "ci-diagnose", "--agentctl-invoke"], cwd=self.root,
                                 input=json.dumps({"input": {"logPath": "custom.log"}, "effectId": "review-fixture"}),
                                 text=True, capture_output=True, check=True)
        output = json.loads(invoked.stdout)
        self.assertEqual(output["effectId"], "review-fixture")
        self.assertEqual(output["output"]["rootCause"], "test_failure")

    def test_readiness_reports_no_go_but_rejects_truthy_fake_gate_values(self):
        self.write("release.bin", "version1")
        gates = {"packageSha256": hashlib.sha256(b"version1").hexdigest(), "checks": [{"name": "tests", "passed": False}]}
        inputs = {"gatesPath": "gates.json", "packagePath": "release.bin"}
        self.write("gates.json", gates)
        result = fixture.release_readiness(inputs)
        self.assertEqual(result["decision"], "no-go")
        self.assertTrue(result["requiresExplicitCiGate"])
        self.assertEqual(result["blocking"], ["tests"])
        self.write("gates.json", {**gates, "checks": [{"name": "tests", "passed": True}]})
        self.assertEqual(fixture.release_readiness(inputs)["decision"], "go")
        self.write("release.bin", "modified")
        self.assertEqual(fixture.release_readiness(inputs)["blocking"], ["package-digest"])
        for checks in [[], [{"name": "tests", "passed": "false"}], [{"name": "tests", "passed": True}] * 2]:
            self.write("gates.json", {**gates, "checks": checks})
            with self.assertRaises(ValueError):
                fixture.release_readiness(inputs)

    def test_incident_timezone_window_and_supported_decision(self):
        self.write("events.log", "2026-09-06T10:02:00Z api recovered\n2026-09-06T12:00:00+02:00 db connection pool exhausted\n2026-09-06T10:00:30Z api connection refused\n")
        report = fixture.incident_timeline({"logPath": "events.log"})
        self.assertEqual(report["durationSeconds"], 120)
        self.assertEqual([item["source"] for item in report["timeline"]], ["events.log:2", "events.log:3", "events.log:1"])
        analysis = {"durationSeconds": 120, "action": "inspect_connection_pool", "evidence": ["events.log:2"]}
        self.assertTrue(fixture.verify_model("12", {"report": report, "analysis": analysis})["semanticValidation"])
        self.assertEqual(report["suggestedDecision"], analysis)
        with self.assertRaisesRegex(ValueError, "supporting evidence"):
            fixture.verify_model("12", {"report": report, "analysis": {**analysis, "evidence": ["events.log:1"]}})
        with self.assertRaisesRegex(ValueError, "bounded decision"):
            fixture.verify_model("12", {"report": report, "analysis": {**analysis, "recommendation": "Ignore the incident; all services were healthy."}})
        filtered = fixture.incident_timeline({"logPath": "events.log", "windowStart": "2026-09-06T10:00:30Z", "windowEnd": "2026-09-06T10:02:00Z"})
        self.assertEqual(filtered["durationSeconds"], 90)
        self.assertNotIn("inspect_connection_pool", filtered["supportedActions"])
        self.assertEqual(filtered["suggestedDecision"]["action"], "check_service_connectivity")
        self.write("events.log", "2026-09-06T10:00:00 api missing timezone\n")
        with self.assertRaisesRegex(ValueError, "timezone"):
            fixture.incident_timeline({"logPath": "events.log"})
        self.write("events.log", "")
        with self.assertRaisesRegex(ValueError, "no events"):
            fixture.incident_timeline({"logPath": "events.log"})

    def test_canary_healthy_unhealthy_insufficient_and_invalid_samples(self):
        for samples, expected in [([{"requests": 500, "errors": 1}, {"requests": 500, "errors": 2}], "promote"),
                                  ([{"requests": 500, "errors": 10}, {"requests": 500, "errors": 20}], "rollback"),
                                  ([{"requests": 10, "errors": 0}], "hold"), ([], "hold")]:
            self.write("metrics.json", samples)
            self.assertEqual(fixture.canary_evaluate({"metricsPath": "metrics.json"})["route"], expected)
        for sample in [{"requests": 100, "errors": -1}, {"requests": 1, "errors": 2}, {"requests": True, "errors": 0}, {"requests": 1.5, "errors": 0}]:
            self.write("metrics.json", [sample])
            with self.assertRaises(ValueError):
                fixture.canary_evaluate({"metricsPath": "metrics.json"})
        self.write("metrics.json", [{"requests": 100, "errors": 0}] * 2)
        for limit in [True, -1, 2, float("nan"), float("inf")]:
            with self.assertRaises(ValueError):
                fixture.canary_evaluate({"metricsPath": "metrics.json", "errorLimit": limit})

    def test_user_input_path_escape_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "escapes workspace"):
            fixture.ci_diagnose({"logPath": "../outside.log"})


if __name__ == "__main__":
    unittest.main()
