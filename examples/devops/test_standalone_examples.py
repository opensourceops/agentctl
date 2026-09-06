"""Independently cataloged examples retain required credential-free verification."""
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parent


class StandaloneExamplesTests(unittest.TestCase):
    def test_standalone_inventory_and_isolated_contracts(self):
        entries = json.loads((ROOT / 'standalone-catalog.json').read_text())['examples']
        self.assertEqual([entry['id'] for entry in entries], ['21'])
        for entry in entries:
            folder = ROOT / entry['directory']
            for field in ['workflow', 'eligibilityWorkflow', 'exporter']:
                self.assertTrue((folder / entry[field]).is_file(), field)
            self.assertTrue((folder / 'README.md').is_file())
            for pattern in entry['contractTests']:
                result = subprocess.run([sys.executable, '-m', 'unittest', 'discover', '-s', 'tests', '-p', pattern], cwd=folder, capture_output=True, text=True, timeout=180)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == '__main__':
    unittest.main()
