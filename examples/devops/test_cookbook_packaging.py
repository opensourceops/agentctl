"""Catalog completeness, isolated public setup, and generated/package freshness."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import fixture
import package
from yaml_io import load, dumps


ROOT = Path(__file__).resolve().parent


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def authored_files(root):
    return {str(path.relative_to(root)): digest(path) for path in root.rglob("*")
            if path.is_file() and "__pycache__" not in path.parts and path.suffix != ".pyc"}


def source_references(value):
    if isinstance(value, dict):
        if "instructionsFile" in value:
            yield value["instructionsFile"]
        yield from value.get("varsFiles", [])
        for child in value.values():
            yield from source_references(child)
    elif isinstance(value, list):
        for child in value:
            yield from source_references(child)


class CookbookPackagingTests(unittest.TestCase):
    def test_catalog_covers_all_twenty_workflows_and_resolves_evidence_paths(self):
        catalog = json.loads((ROOT / "catalog.json").read_text(encoding="utf-8"))
        entries = catalog["examples"]
        self.assertEqual([entry["id"] for entry in entries], [f"{number:02}" for number in range(1, 21)])
        self.assertEqual({entry["directory"] for entry in entries},
                         {path.name for path in ROOT.glob("[0-9][0-9]-*") if path.is_dir()})
        for entry in entries:
            with self.subTest(example=entry["id"]):
                folder = ROOT / entry["directory"]
                listed = {value for key, value in entry.items() if key.lower().endswith("workflow") and value}
                actual = {path.name for path in folder.glob("*.yaml") if load(path).get("kind") == "Workflow"}
                self.assertEqual(listed, actual, "every authored variant needs a catalog entry")
                self.assertEqual(entry["expectedExitCodes"]["completed"], 0)
                self.assertEqual(set(entry["platforms"]), {"linux", "macos", "windows"})
                self.assertTrue((ROOT / entry["pythonRequirements"]).is_file())
                self.assertEqual(entry["evidence"]["pathBase"], "examples/devops")
                for field in ("validationFile", "reviewLedger", "directCommands"):
                    self.assertTrue((ROOT / entry["evidence"][field]).is_file(), field)
                self.assertIn("historical", entry["evidence"]["validationScope"])
                for name in listed:
                    workflow = load(folder / name)
                    self.assertFalse((folder / name).read_text(encoding="utf-8").lstrip().startswith("{"))
                    for reference in source_references(workflow):
                        self.assertTrue((folder / reference).is_file(), reference)
                    for action in workflow["spec"]["actions"].values():
                        if action["kind"] == "extension.process":
                            self.assertEqual(action["args"][0], "helper.py")
                            operation = action["args"][1]
                            self.assertIn(operation, fixture.OPERATIONS)
                            self.assertEqual(action["inputSchema"], fixture.operation_input_schema(operation))
                            self.assertEqual(action["outputSchema"], fixture.operation_output_schema(operation))

    def test_generator_is_fresh_repeatable_and_preserves_editorial_readmes(self):
        # Generation runs only in a disposable copy, never in the checkout being
        # tested. Comparing the first run catches stale committed generated files.
        with tempfile.TemporaryDirectory(prefix="agentctl-cookbook-generation-") as directory:
            copied = Path(directory) / "devops"
            shutil.copytree(ROOT, copied, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "artifacts", "evidence", "local.*.yaml"))
            tutorial = copied / "03-pipeline-review/README.md"
            editorial = tutorial.read_bytes() + b"\nHand-maintained editorial sentinel.\n"
            tutorial.write_bytes(editorial)
            before = authored_files(copied)
            for iteration in range(2):
                result = subprocess.run([sys.executable, str(copied / "build_catalog.py")], cwd=copied,
                                        capture_output=True, text=True, timeout=60)
                self.assertEqual(result.returncode, 0, result.stderr)
                after = authored_files(copied)
                changed = sorted(name for name in before.keys() | after.keys() if before.get(name) != after.get(name))
                self.assertEqual(changed, [], f"generation {iteration + 1} changed checked-in outputs; regenerate examples/devops/build_catalog.py")
                self.assertEqual(tutorial.read_bytes(), editorial)
                before = after

    def test_every_package_contains_declared_support_and_verified_source_assets(self):
        with tempfile.TemporaryDirectory(prefix="agentctl-cookbook-packages-") as directory:
            for identifier in (f"{number:02}" for number in range(1, 21)):
                destination = Path(directory) / identifier
                metadata = package.package_example(identifier, destination)
                with self.subTest(example=identifier):
                    self.assertTrue(set(package.SUPPORT.values()).issubset(metadata["files"]))
                    self.assertEqual(metadata["exampleId"], identifier)
                    self.assertRegex(metadata["sourceSha"], r"^[0-9a-f]{40}$")
                    for name, expected in metadata["files"].items():
                        self.assertEqual(digest(destination / name), expected)
                    workflow = load(destination / "workflow.yaml")
                    for reference in source_references(workflow):
                        self.assertTrue((destination / reference).is_file(), reference)
                    self.assertFalse(list(destination.glob("local.*.yaml")))
                    self.assertFalse((destination / "state.db").exists())

    def test_public_setup_preserves_source_and_grants_actual_interpreter_basename(self):
        with tempfile.TemporaryDirectory(prefix="agentctl-cookbook-setup-") as directory:
            destination = Path(directory) / "package"
            package.package_example("03", destination)
            source_hash = digest(destination / "workflow.yaml")
            environment = os.environ.copy()
            environment.pop("PYTHONPATH", None)
            result = subprocess.run([sys.executable, str(destination / "setup.py")], cwd=destination,
                                    env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            workflow = load(destination / "local.workflow.yaml")
            self.assertEqual(source_hash, digest(destination / "workflow.yaml"))
            for action in workflow["spec"]["actions"].values():
                if action["kind"] == "extension.process":
                    executable = Path(action["command"])
                    self.assertTrue(executable.is_absolute())
                    self.assertIn(executable.name, workflow["spec"]["policy"]["processAllowlist"])
            self.assertFalse((destination / "artifacts").exists(), "setup must not execute this workflow")

    def test_zip_bytes_are_deterministic_and_existing_work_is_not_replaced(self):
        with tempfile.TemporaryDirectory(prefix="agentctl-cookbook-archives-") as directory:
            root = Path(directory)
            package.package_example("03", root / "first", root / "first.zip")
            package.package_example("03", root / "second", root / "second.zip")
            self.assertEqual(digest(root / "first.zip"), digest(root / "second.zip"))
            before = authored_files(root / "first")
            with self.assertRaisesRegex(ValueError, "existing work"):
                package.package_example("03", root / "first")
            self.assertEqual(before, authored_files(root / "first"))

    @unittest.skipUnless(os.environ.get("AGENTCTL_EXAMPLES_BINARY"), "xtask supplies the built CLI for compiler equivalence")
    def test_json_to_readable_yaml_preserves_actual_compiled_digests(self):
        value = {"apiVersion": "agentctl.dev/v1", "kind": "Workflow", "metadata": {"name": "yaml-equivalence"},
                 "spec": {"vars": {"items": [True, None, 42, "42"], "text": "café\nline two\n", "object": {"enabled": False}},
                          "actions": {"assign": {"kind": "builtin.assign"}},
                          "tasks": [{"id": "assign", "uses": "action:assign", "with": {"items": "${{ vars.items }}", "text": "${{ vars.text }}", "object": "${{ vars.object }}"}}]}}
        with tempfile.TemporaryDirectory(prefix="agentctl-cookbook-digests-") as directory:
            workflow = Path(directory) / "workflow.yaml"
            results = []
            for text in (json.dumps(value, ensure_ascii=False), dumps(value)):
                workflow.write_bytes(text.encode("utf-8"))
                result = subprocess.run([os.environ["AGENTCTL_EXAMPLES_BINARY"], "check", str(workflow), "--output", "json"],
                                        cwd=directory, capture_output=True, text=True, timeout=30)
                self.assertEqual(result.returncode, 0, result.stderr)
                results.append(json.loads(result.stdout)["data"])
            self.assertEqual(results[0]["workflowDigest"], results[1]["workflowDigest"])
            self.assertEqual(results[0]["planDigest"], results[1]["planDigest"])


if __name__ == "__main__":
    unittest.main()
