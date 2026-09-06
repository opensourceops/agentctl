import tempfile
from pathlib import Path
import unittest

from ruamel.yaml.constructor import DuplicateKeyError
import yaml_io


class YamlAuthoringTests(unittest.TestCase):
    def test_round_trip_preserves_authored_json_types_and_exact_text(self):
        value = {"null": None, "boolean": True, "integer": 42, "float": 0.05,
                 "array": [False, None, "null", "true", "01", "on", "2026-09-06"],
                 "template": "${{ inputs.threshold }}", "embeddedJson": '{"done":true}',
                 "instructions": "Read café evidence.\nPreserve the final newline.\n",
                 "withoutFinalNewline": "first\nsecond", "empty": "", "object": {}}
        encoded = yaml_io.dumps(value)
        self.assertEqual(yaml_io.loads(encoded), value)
        self.assertNotIn("missing", yaml_io.loads(encoded))
        self.assertFalse(encoded.startswith("{"))
        self.assertIn("instructions: |\n", encoded)
        self.assertIn("  - false\n", encoded)

    def test_yaml_12_keeps_github_actions_on_as_a_string_key(self):
        self.assertEqual(yaml_io.loads("on: [push]\nyes: no\n"), {"on": ["push"], "yes": "no"})

    def test_duplicate_keys_and_unsupported_values_are_rejected(self):
        with self.assertRaises(DuplicateKeyError):
            yaml_io.loads("spec:\n  vars:\n    count: 1\n    count: 2\n")
        for text in ["1: value\n", "value: .nan\n", "value: !!python/object:object {}\n"]:
            with self.subTest(text=text), self.assertRaises(Exception):
                yaml_io.loads(text)

    def test_json_is_accepted_as_input_but_negative_workflows_are_written_as_yaml(self):
        with tempfile.TemporaryDirectory() as directory:
            filename = Path(directory) / "denied.workflow.yaml"
            value = yaml_io.loads('{"spec":{"policy":{"approval":"never"}}}')
            yaml_io.dump(filename, value, "Expected policy denial; no forbidden output.")
            self.assertEqual(yaml_io.load(filename), value)
            self.assertIn("spec:\n", filename.read_text())

    def test_serialization_is_stable_across_repeated_generation(self):
        source = "name: café\nsteps:\n  - with:\n      input: '${{ inputs.source }}'\n"
        first = yaml_io.dumps(yaml_io.loads(source))
        self.assertEqual(first, yaml_io.dumps(yaml_io.loads(first)))


if __name__ == "__main__":
    unittest.main()
