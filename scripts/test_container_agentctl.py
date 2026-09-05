import unittest

from container_agentctl import command


class ContainerAdapterTests(unittest.TestCase):
    def test_only_real_dispatch_passes_credential_by_name(self):
        configuration = {"program": "podman", "args": ["run", "--rm"]}
        for args in (["migrate", "/config/workflow.yaml"], ["inspect", "run-1"],
                     ["replay", "run-1"], ["run", "/config/workflow.yaml", "--check"],
                     ["repair", "/config/fix.yaml", "run-1", "--plan"]):
            with self.subTest(args=args):
                output = command(args, configuration)
                self.assertIn("--network", output)
                self.assertIn("none", output)
                self.assertNotIn("--env", output)
        output = command(["run", "/config/workflow.yaml", "--output", "json"], configuration)
        self.assertEqual(output[3:5], ["--env", "OPENAI_API_KEY"])
        self.assertNotIn("--network", output)


if __name__ == "__main__":
    unittest.main()
