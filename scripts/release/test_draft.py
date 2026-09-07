"""Exercise first-draft discovery and safe upload reconciliation with a GitHub fixture."""
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import draft


class DraftTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.contents = {'release-bundle.json': b'{"fixture":true}\n',
                         'release-bundle.sigstore.json': b'fixture signature\n',
                         'fixture.zip': b'\x00fixture package\n'}
        for name, contents in self.contents.items():
            (self.directory / name).write_bytes(contents)
        self.source = 'a' * 40
        self.record = None
        self.remote = {}
        self.created = 0
        self.uploads = []
        self.reads = []
        self.after_create = None
        self.signature_verified = False

    def github(self, *args):
        self.assertTrue(self.signature_verified, 'remote operations require the attestation gate')
        if args[:1] == ('api',):
            self.reads.append(args)
            if '/releases/tags/' in args[-1]:
                # Hosted preparation 34155764812: creation succeeded, but this
                # endpoint returned 404 for the unpublished draft.
                raise subprocess.CalledProcessError(1, ['gh', *args], stderr='Not Found (HTTP 404)')
            self.assertEqual(args, ('api', '--paginate', '--slurp',
                                    'repos/opensourceops/agentctl/releases?per_page=100'))
            unrelated = {'id': 1, 'tag_name': 'v0.3.0', 'draft': False}
            matching = [self.record] if self.record else []
            if self.created and self.after_create:
                matching = self.after_create(self.record)
            return json.dumps([[unrelated], matching])
        self.assertEqual(args[0], 'release')
        self.assertEqual(args[2], 'v0.4.0')
        self.assertIn('--repo', args)
        self.assertEqual(args[args.index('--repo') + 1], draft.REPO)
        if args[1] == 'create':
            self.assertIsNone(self.record, 'never duplicate a known draft')
            self.assertIn('--verify-tag', args)
            self.assertIn('--draft', args)
            notes = Path(args[args.index('--notes-file') + 1]).read_text()
            self.assertIn(self.source, notes)
            self.assertIn('/actions/runs/123', notes)
            self.record = {'id': 384291529, 'tag_name': 'v0.4.0', 'draft': True,
                           'prerelease': False, 'assets': [],
                           'html_url': 'https://github.com/opensourceops/agentctl/releases/tag/untagged-fixture'}
            self.created += 1
            return self.record['html_url'] + '\n'
        if args[1] == 'upload':
            self.assertEqual(self.record['id'], 384291529)
            self.assertNotIn('--clobber', args)
            path = Path(args[3])
            self.assertNotIn(path.name, self.remote)
            self.remote[path.name] = path.read_bytes()
            self.record['assets'].append({'name': path.name, 'id': len(self.remote)})
            self.uploads.append(path.name)
            return ''
        self.assertEqual(args[1], 'download')
        name = args[args.index('--pattern') + 1]
        (Path(args[args.index('--dir') + 1]) / name).write_bytes(self.remote[name])
        return ''

    def attestation(self, args, **kwargs):
        self.assertEqual(args[:3], ['gh', 'attestation', 'verify'])
        for option in ['--source-digest', '--signer-digest']:
            self.assertEqual(args[args.index(option) + 1], self.source)
        self.assertEqual(args[args.index('--repo') + 1], draft.REPO)
        self.assertEqual(args[args.index('--signer-workflow') + 1],
                         draft.REPO + '/.github/workflows/release-prep.yml')
        self.assertIn('--deny-self-hosted-runners', args)
        self.assertTrue(kwargs['check'])
        self.signature_verified = True
        return subprocess.CompletedProcess(args, 0)

    def execute(self, tag_source=None, signature=None):
        argv = ['draft.py', '--source', self.source, '--tag', 'v0.4.0',
                '--directory', str(self.directory)]
        output = io.StringIO()
        with patch.object(sys, 'argv', argv), redirect_stdout(output), \
             patch.object(draft, 'identity', return_value={'prerelease': False}), \
             patch.object(draft, 'verify', return_value={'preparationRunId': '123'}), \
             patch.object(draft.subprocess, 'check_output', return_value=(tag_source or self.source) + '\n'), \
             patch.object(draft.subprocess, 'run', side_effect=signature or self.attestation), \
             patch.object(draft, 'gh', side_effect=self.github):
            draft.main()
        return output.getvalue().strip()

    def test_new_draft_found_on_later_page_when_tag_endpoint_is_unavailable(self):
        url = self.execute()
        self.assertEqual(url, self.record['html_url'])
        self.assertEqual(self.created, 1)
        self.assertEqual(len(self.reads), 2)
        self.assertEqual(self.remote, self.contents)
        self.assertEqual(set(self.uploads), set(self.contents))
        self.assertFalse((self.directory / 'release-notes.md').exists())

    def test_existing_identical_draft_is_reconciled_without_creation_or_upload(self):
        self.execute()
        uploads = list(self.uploads)
        self.execute()
        self.assertEqual(self.created, 1)
        self.assertEqual(self.uploads, uploads)
        self.assertEqual(self.remote, self.contents)

    def test_changed_existing_asset_is_preserved_and_rejected(self):
        self.execute()
        self.remote['fixture.zip'] = b'different remote bytes'
        uploads = list(self.uploads)
        with self.assertRaisesRegex(ValueError, 'existing draft asset differs'):
            self.execute()
        self.assertEqual(self.uploads, uploads)
        self.assertEqual(self.remote['fixture.zip'], b'different remote bytes')

    def test_missing_created_draft_is_not_recreated_or_uploaded(self):
        self.after_create = lambda record: []
        with self.assertRaisesRegex(ValueError, 'created draft could not be located'):
            self.execute()
        self.assertEqual(self.created, 1)
        self.assertEqual(self.uploads, [])

    def test_ambiguous_created_draft_cannot_receive_assets(self):
        self.after_create = lambda record: [record, dict(record, id=384291530)]
        with self.assertRaisesRegex(ValueError, 'ambiguous release tag'):
            self.execute()
        self.assertEqual(self.created, 1)
        self.assertEqual(self.uploads, [])

    def test_published_or_mismatched_prerelease_record_cannot_receive_assets(self):
        for changed, message in [({'draft': False}, 'already published'),
                                 ({'prerelease': True}, 'prerelease setting disagrees')]:
            with self.subTest(changed=changed):
                self.record = {'id': 384291529, 'tag_name': 'v0.4.0', 'draft': True,
                               'prerelease': False, 'assets': [], **changed}
                with self.assertRaisesRegex(ValueError, message):
                    self.execute()
                self.assertEqual(self.created, 0)
                self.assertEqual(self.uploads, [])

    def test_tag_or_signature_failure_prevents_all_remote_operations(self):
        with self.assertRaisesRegex(ValueError, 'exact matching tag'):
            self.execute(tag_source='b' * 40)
        def invalid_signature(args, **kwargs):
            raise subprocess.CalledProcessError(1, args, stderr='invalid attestation')
        with self.assertRaises(subprocess.CalledProcessError):
            self.execute(signature=invalid_signature)
        self.assertEqual(self.reads, [])
        self.assertEqual(self.created, 0)
        self.assertEqual(self.uploads, [])


if __name__ == '__main__':
    unittest.main()
