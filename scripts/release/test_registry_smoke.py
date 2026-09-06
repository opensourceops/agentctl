"""Model Docker's classic-store digest conflict without a registry or Docker daemon."""
import json
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


class RegistrySmokeTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.source = self.root / 'release-bundle.json'
        self.output = self.root / 'registry-smoke.json'
        self.registry = '127.0.0.1:5000/agentctl'
        self.images = [
            {'variant': variant, 'manifestDigest': 'sha256:' + index * 64,
             'platforms': [{'platform': 'linux/' + arch, 'configDigest': 'sha256:' + digest * 64}
                           for arch, digest in [('amd64', amd), ('arm64', arm)]]}
            for variant, index, amd, arm in [('minimal', '1', 'a', 'b'), ('tooling', '2', 'c', 'd')]
        ]
        self.source.write_text(json.dumps({'images': self.images}))
        self.local = {}
        self.pulled = []
        self.removed = []
        self.bad_config = False

    def run_command(self, args, **kwargs):
        self.assertEqual(args[0], 'docker')
        self.assertTrue(kwargs['check'])
        if args[1] == 'pull':
            self.assertEqual(args[2], '--platform')
            platform, reference = args[3:]
            # Actual Docker 28 classic-store failure observed in hosted run
            # 34054932901 attempt 2: the same index cannot name two local images.
            if reference in self.local:
                raise subprocess.CalledProcessError(1, args, stderr='cannot overwrite digest')
            variant = next(image for image in self.images
                           if reference == self.registry + '@' + image['manifestDigest'])
            item = next(item for item in variant['platforms'] if item['platform'] == platform)
            self.local[reference] = {'Id': item['configDigest'], 'Os': 'linux',
                                     'Architecture': platform.split('/')[1]}
            self.pulled.append((variant['variant'], platform))
        else:
            self.assertEqual(args[1:3], ['image', 'rm'])
            self.assertEqual(len(args), 4)  # No force, prune, or unrelated names.
            reference = args[3]
            self.assertIn(reference, self.local)
            del self.local[reference]
            self.removed.append(reference)
        return subprocess.CompletedProcess(args, 0)

    def inspect(self, args):
        self.assertEqual(args[:3], ['docker', 'image', 'inspect'])
        value = dict(self.local[args[3]])
        if self.bad_config:
            value['Id'] = 'sha256:' + 'f' * 64
        return json.dumps([value]).encode()

    def execute(self, runner=None):
        args = ['registry_smoke.py', '--bundle', str(self.source), '--registry', self.registry,
                '--output', str(self.output)]
        with patch.object(sys, 'argv', args), \
             patch.object(subprocess, 'run', side_effect=runner or self.run_command), \
             patch.object(subprocess, 'check_output', side_effect=self.inspect):
            runpy.run_path(str(Path(__file__).with_name('registry_smoke.py')), run_name='__main__')

    def test_both_platforms_use_full_index_and_release_only_their_local_reference(self):
        self.execute()
        self.assertEqual(self.pulled, [('minimal', 'linux/amd64'), ('minimal', 'linux/arm64'),
                                       ('tooling', 'linux/amd64'), ('tooling', 'linux/arm64')])
        self.assertEqual(len(self.removed), 4)
        self.assertEqual(self.local, {})
        report = json.loads(self.output.read_text())
        self.assertTrue(report['passed'])
        self.assertEqual(len(report['pulls']), 4)

    def test_wrong_delivered_config_still_fails_and_releases_the_pulled_reference(self):
        self.bad_config = True
        with self.assertRaisesRegex(ValueError, 'tested native platform/config'):
            self.execute()
        self.assertEqual(len(self.pulled), 1)
        self.assertEqual(len(self.removed), 1)
        self.assertEqual(self.local, {})
        self.assertFalse(self.output.exists())

    def test_failed_pull_does_not_claim_coverage_or_remove_an_unacquired_reference(self):
        def denied(args, **kwargs):
            self.assertEqual(args[1], 'pull')
            raise subprocess.CalledProcessError(1, args, stderr='registry unavailable')
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(denied)
        self.assertFalse(self.output.exists())
        self.assertEqual(self.removed, [])


if __name__ == '__main__':
    unittest.main()
