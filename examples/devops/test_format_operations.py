"""Input-derived regression checks; no clusters, Terraform apply, or registries."""
import copy
import difflib
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

import fixture
import format_operations as formats
from yaml_io import dumps, loads


ROOT = Path(__file__).resolve().parent


class FormatOperationsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.old_root, fixture.ROOT = fixture.ROOT, Path(self.temp.name).resolve()
        from jsonschema import Draft202012Validator
        def checked(name, operation, payload):
            Draft202012Validator(formats.INPUT_SCHEMAS[name]).validate(payload)
            result = operation(payload)
            Draft202012Validator(formats.OUTPUT_SCHEMAS[name]).validate({**result, "reportText": json.dumps(result)})
            return result
        self.ops = {name: (lambda payload, name=name, operation=operation: checked(name, operation, payload))
                    for name, operation in formats.register(fixture).items()}

    def tearDown(self):
        fixture.ROOT = self.old_root
        self.temp.cleanup()

    def pipeline(self):
        return {"name": "User build", "on": {"push": {"branches": ["feature"]}},
                "concurrency": {"group": "ci-${{ github.ref }}", "cancel-in-progress": True},
                "permissions": "write-all", "env": {"USER_VALUE": "preserve"},
                "jobs": {"test": {"runs-on": "ubuntu-latest", "timeout-minutes": 90,
                                   "steps": [{"run": "python3 -m unittest", "env": {"LANG": "C.UTF-8"}}]}}}

    def pipeline_proposal(self, value=None):
        fixture.write("input.yaml", dumps(value or self.pipeline()))
        return self.ops["pipeline-propose"]({"sourcePath": "input.yaml", "maxTimeoutMinutes": 10})

    def validate_pipeline(self, proposed, **extra):
        return self.ops["pipeline-validate"]({"sourcePath": "input.yaml", "proposalPath": proposed["proposalPath"],
                                              "maxTimeoutMinutes": 10, "requireActionlint": False,
                                              "actionlintPath": "tools/actionlint", **extra})

    def test_pipeline_preserves_trigger_steps_environment_and_concurrency(self):
        original = self.pipeline()
        proposed = self.pipeline_proposal(original)
        final = self.validate_pipeline(proposed)
        actual = loads(fixture.read("artifacts/patch-workspace/pipeline.yaml"))
        expected = copy.deepcopy(original)
        expected["permissions"] = {"contents": "read"}
        expected["jobs"]["test"]["timeout-minutes"] = 10
        self.assertEqual(actual, expected)
        self.assertEqual(final["actionlint"]["status"], "unverified")
        self.assertEqual(len(final["violations"]), 2)

    def test_pipeline_rejects_proposal_that_changes_an_unrelated_step(self):
        proposed = self.pipeline_proposal()
        value = loads(fixture.read(proposed["proposalPath"]))
        value["jobs"]["test"]["steps"] = [{"run": "echo skip-tests"}]
        fixture.write(proposed["proposalPath"], dumps(value))
        with self.assertRaisesRegex(ValueError, "unrelated"):
            self.validate_pipeline(proposed)
        self.assertFalse(fixture.path("artifacts/patch-workspace/pipeline.yaml").exists())

    def test_pipeline_checks_each_jobs_permissions_and_requires_real_on_key(self):
        value = self.pipeline()
        value["jobs"]["test"]["permissions"] = {"id-token": "write"}
        proposed = self.pipeline_proposal(value)
        self.assertEqual(len(self.validate_pipeline(proposed)["violations"]), 3)
        del value["on"]
        with self.assertRaisesRegex(ValueError, "GitHub Actions"):
            self.pipeline_proposal(value)

    def test_compliant_pipeline_accepts_an_empty_patch(self):
        value = self.pipeline()
        value["permissions"] = {"contents": "read"}
        value["jobs"]["test"]["timeout-minutes"] = 10
        result = self.validate_pipeline(self.pipeline_proposal(value))
        self.assertEqual(result["violations"], [])
        self.assertFalse(result["patch"]["changed"])

    @unittest.skipUnless(os.environ.get("ACTIONLINT_TEST_BINARY"), "set ACTIONLINT_TEST_BINARY for actual pinned validator evidence")
    def test_actual_actionlint_accepts_workflow_and_rejects_unknown_expression(self):
        fixture.path("tools").mkdir()
        shutil.copy2(os.environ["ACTIONLINT_TEST_BINARY"], fixture.path("tools/actionlint"))
        result = self.validate_pipeline(self.pipeline_proposal(), requireActionlint=True)
        self.assertEqual(result["actionlint"], {"status": "passed", "version": "1.7.7",
                         "scope": "actionlint syntax, expressions and built-in semantic checks; shellcheck and pyflakes disabled"})
        value = self.pipeline()
        value["jobs"]["test"]["steps"][0]["run"] = "echo '${{ imaginary.output }}'"
        with self.assertRaises(ValueError):
            self.validate_pipeline(self.pipeline_proposal(value), requireActionlint=True)

    def test_dockerfile_preserves_labels_and_never_runs_supplied_app(self):
        before = 'FROM python:3.12-slim\nLABEL owner="user"\nWORKDIR /app\nCOPY . /app\nUSER root\nCMD ["python3", "app.py"]\n'
        fixture.write("Dockerfile", before)
        fixture.write("app.py", "raise RuntimeError('would execute')\n")
        proposed = self.ops["dockerfile-propose"]({"sourcePath": "Dockerfile"})
        result = self.ops["dockerfile-validate"]({"sourcePath": "Dockerfile", "proposalPath": proposed["proposalPath"], "appPath": "app.py"})
        self.assertTrue(result["syntaxValidated"])
        self.assertIsNone(result["performanceClaim"])
        self.assertEqual(result["containerBuild"], "not-executed; use the documented optional image gate")
        actual = fixture.read("artifacts/Dockerfile")
        self.assertIn('LABEL owner="user"', actual)
        self.assertIn('CMD ["python3", "app.py"]', actual)
        self.assertIn("COPY app.py /app/app.py", actual)
        self.assertIn("USER 65534:65534", actual)
        fixture.write("app.py", "def broken(:\n")
        with self.assertRaises(SyntaxError):
            self.ops["dockerfile-validate"]({"sourcePath": "Dockerfile", "proposalPath": proposed["proposalPath"], "appPath": "app.py"})

    def deployment(self):
        return {"apiVersion": "apps/v1", "kind": "Deployment",
                "metadata": {"name": "user-api", "namespace": "preview", "annotations": {"user": "preserve"}},
                "spec": {"replicas": "2", "selector": {"matchLabels": {"app": "api"}},
                         "strategy": {"type": "RollingUpdate", "rollingUpdate": {"maxSurge": 1}},
                         "template": {"metadata": {"labels": {"app": "api", "team": "platform"}},
                                      "spec": {"containers": [{"name": "api", "image": "example.invalid/api:fixture",
                                               "ports": [{"containerPort": 8080}], "env": [{"name": "MODE", "value": "preview"}],
                                               "resources": {"requests": {"cpu": "10m"}},
                                               "securityContext": {"readOnlyRootFilesystem": True, "runAsUser": 1000}}]}}}}

    def kubernetes_proposal(self, value=None):
        fixture.write("input.yaml", dumps(value or self.deployment()))
        fixture.write("deployment.schema.json", (ROOT / "schemas/deployment-v1.35.0.schema.json").read_text())
        return self.ops["kubernetes-propose"]({"sourcePath": "input.yaml"})

    def validate_kubernetes(self, proposed):
        return self.ops["kubernetes-validate"]({"sourcePath": "input.yaml", "proposalPath": proposed["proposalPath"], "schemaPath": "deployment.schema.json"})

    def test_full_deployment_schema_preserves_fields_and_accepts_numeric_intorstring(self):
        original = self.deployment()
        result = self.validate_kubernetes(self.kubernetes_proposal(original))
        expected = copy.deepcopy(original)
        expected["spec"]["replicas"] = 2
        expected["spec"]["template"]["spec"]["containers"][0]["securityContext"].update(runAsNonRoot=True, allowPrivilegeEscalation=False)
        self.assertEqual(result["manifest"], expected)
        self.assertEqual(result["schemaSha256"], formats.KUBERNETES_SCHEMA_SHA256)

    def test_full_schema_rejects_invalid_port_outside_old_subset(self):
        from jsonschema import ValidationError
        value = self.deployment()
        value["spec"]["template"]["spec"]["containers"][0]["ports"][0]["containerPort"] = "eighty"
        with self.assertRaises(ValidationError):
            self.validate_kubernetes(self.kubernetes_proposal(value))
        self.assertFalse(fixture.path("artifacts/deployment.json").exists())

    def test_kubernetes_rejects_schema_tamper_and_unrelated_proposal_changes(self):
        proposed = self.kubernetes_proposal()
        fixture.write("deployment.schema.json", {"type": "object"})
        with self.assertRaisesRegex(ValueError, "pin"):
            self.validate_kubernetes(proposed)
        proposed = self.kubernetes_proposal()
        value = loads(fixture.read(proposed["proposalPath"]))
        value["metadata"]["namespace"] = "production"
        fixture.write(proposed["proposalPath"], dumps(value))
        with self.assertRaisesRegex(ValueError, "unrelated"):
            self.validate_kubernetes(proposed)

    def plan(self):
        return {"format_version": "1.2", "terraform_version": "1.10.5",
                "planned_values": {"root_module": {"resources": []}},
                "resource_changes": [{"address": "local_file.fixture", "mode": "managed", "type": "local_file",
                    "name": "fixture", "provider_name": "registry.terraform.io/hashicorp/local",
                    "change": {"actions": ["create"], "before": None,
                               "after": {"filename": "artifacts/local-state.json", "content": '{"value":"input-derived"}\n'},
                               "after_unknown": {"id": True, "content_sha256": True}, "before_sensitive": False, "after_sensitive": {}}}]}

    def analyze_plan(self, value):
        fixture.write("plan.json", value)
        return self.ops["terraform-analyze"]({"planPath": "plan.json", "allowedFilename": "artifacts/local-state.json"})

    def test_real_plan_shape_returns_exact_reviewed_content_without_mutation(self):
        result = self.analyze_plan(self.plan())
        self.assertTrue(result["allowed"])
        self.assertEqual(json.loads(result["desiredContent"]), {"value": "input-derived"})
        self.assertFalse(fixture.path("artifacts/local-state.json").exists())

    def test_plan_rejects_deletion_spoofed_address_other_target_unknown_and_sensitive(self):
        for field, value in [("actions", ["delete", "create"]), ("after_unknown", {"content": True}), ("after_sensitive", {"content": True})]:
            plan = self.plan()
            plan["resource_changes"][0]["change"][field] = value
            self.assertFalse(self.analyze_plan(plan)["allowed"])
        plan = self.plan()
        plan["resource_changes"][0]["type"] = "aws_instance"
        self.assertFalse(self.analyze_plan(plan)["allowed"])
        plan = self.plan()
        plan["resource_changes"][0]["change"]["after"]["filename"] = "production-state.json"
        result = self.analyze_plan(plan)
        self.assertFalse(result["allowed"])
        self.assertEqual(result["desiredContent"], "")
        self.assertFalse(fixture.path("production-state.json").exists())

    def prepare_vendor(self):
        before = 'def major(value):\n    return int(value[0])\n'
        after = 'def major(value):\n    return int(value.split(".", 1)[0])\n'
        fixture.write("source/vendor_version.py", before)
        fixture.write("source/test_dependency.py", 'import unittest\nfrom vendor_version import major\nclass Tests(unittest.TestCase):\n    def test_single(self): self.assertEqual(major("2.3"), 2)\n    def test_double(self): self.assertEqual(major("12.3"), 12)\n    def test_invalid(self):\n        with self.assertRaises(ValueError): major("x.0")\n')
        fixture.write("update.patch", "".join(difflib.unified_diff(before.splitlines(True), after.splitlines(True), fromfile="a/vendor_version.py", tofile="b/vendor_version.py")))
        return self.ops["vendor-prepare"]({"targetPath": "source/vendor_version.py", "testPath": "source/test_dependency.py", "patchPath": "update.patch"})

    def test_supplied_vendored_patch_fails_before_passes_after_and_reuses_applied_bytes(self):
        # A copied example can itself live beneath a developer's Git checkout.
        fixture.command(["git", "init", "--quiet"], cwd=fixture.ROOT)
        prepared = self.prepare_vendor()
        fixture.write("artifacts/patch-workspace/test_stale.py", "raise RuntimeError('stale unrelated test must not run')\n")
        baseline = self.ops["vendor-test-before"]({key: prepared[key] for key in ["targetName", "sourceSha256", "testName", "testSha256"]})
        payload = {key: prepared[key] for key in ["targetName", "sourceSha256", "proposedSha256", "patchSha256"]}
        self.assertFalse(self.ops["vendor-apply"](payload)["reused"])
        self.assertTrue(self.ops["vendor-apply"](payload)["reused"])
        result = self.ops["vendor-test-after"]({**{key: prepared[key] for key in ["targetName", "proposedSha256", "testName", "testSha256"]}, "baselineTests": baseline["tests"]})
        self.assertEqual((result["baselineExit"], result["updatedExit"], result["tests"]), (1, 0, 3))
        self.assertIn("return int(value[0])", fixture.read("source/vendor_version.py"))

    def test_vendor_rejects_changed_patch_and_assertion_free_import_failure(self):
        prepared = self.prepare_vendor()
        fixture.write("artifacts/proposed.patch", "tampered")
        with self.assertRaisesRegex(ValueError, "patch changed"):
            self.ops["vendor-apply"]({key: prepared[key] for key in ["targetName", "sourceSha256", "proposedSha256", "patchSha256"]})
        fixture.write("source/test_dependency.py", "import definitely_missing_test_dependency\n")
        prepared = self.ops["vendor-prepare"]({"targetPath": "source/vendor_version.py", "testPath": "source/test_dependency.py", "patchPath": "update.patch"})
        with self.assertRaisesRegex(ValueError, "behavioral tests"):
            self.ops["vendor-test-before"]({key: prepared[key] for key in ["targetName", "sourceSha256", "testName", "testSha256"]})

    def test_vendor_rejects_changed_test_bytes_even_with_the_same_test_count(self):
        prepared = self.prepare_vendor()
        baseline = self.ops["vendor-test-before"]({key: prepared[key] for key in ["targetName", "sourceSha256", "testName", "testSha256"]})
        self.ops["vendor-apply"]({key: prepared[key] for key in ["targetName", "sourceSha256", "proposedSha256", "patchSha256"]})
        changed = fixture.read("artifacts/patch-workspace/test_dependency.py").replace('major("12.3"), 12', 'True, True')
        fixture.write("artifacts/patch-workspace/test_dependency.py", changed)
        with self.assertRaisesRegex(ValueError, "test bytes changed"):
            self.ops["vendor-test-after"]({**{key: prepared[key] for key in ["targetName", "proposedSha256", "testName", "testSha256"]}, "baselineTests": baseline["tests"]})

    def test_vendor_patch_cannot_silently_modify_another_module(self):
        self.prepare_vendor()
        extra = "--- /dev/null\n+++ b/another.py\n@@ -0,0 +1 @@\n+changed = True\n"
        fixture.write("update.patch", fixture.read("update.patch") + extra)
        with self.assertRaisesRegex(ValueError, "only the named"):
            self.ops["vendor-prepare"]({"targetPath": "source/vendor_version.py", "testPath": "source/test_dependency.py", "patchPath": "update.patch"})
        self.assertFalse(fixture.path("artifacts/proposal-workspace/another.py").exists())

    def test_named_input_contracts_reject_unknown_fields_and_missing_paths(self):
        from jsonschema import Draft202012Validator, ValidationError
        self.assertEqual(set(formats.INPUT_SCHEMAS), set(self.ops))
        for value in formats.INPUT_SCHEMAS.values():
            self.assertFalse(value["additionalProperties"])
            with self.assertRaises(ValidationError):
                Draft202012Validator(value).validate({})


if __name__ == "__main__":
    unittest.main()
