import unittest
import os
from pathlib import Path
import subprocess
import tempfile
from unittest.mock import patch

import registry_preflight as preflight


PUBLIC = {'namespace': 'opensourceops', 'name': 'agentctl', 'is_private': False}
AUTH = (200, {'access_token': 'fixture-bearer'})


class RegistryPreflightTests(unittest.TestCase):
    def run_check(self, responses, create=False):
        with patch.object(preflight, 'request', side_effect=responses) as request:
            result = preflight.check('fixture-user', 'fixture-secret', create)
            return result, request.call_args_list

    def test_existing_public_repository_is_read_only(self):
        result, calls = self.run_check([AUTH, (200, PUBLIC), (200, PUBLIC)], True)
        self.assertFalse(result['created'])
        self.assertEqual([call.args[0] for call in calls], ['POST', 'GET', 'GET'])
        self.assertEqual(calls[-1].args, ('GET', preflight.REPOSITORY))

    def test_missing_repository_requires_explicit_creation_and_exact_public_target(self):
        with patch.object(preflight, 'request', side_effect=[AUTH, (404, None)]) as request:
            with self.assertRaisesRegex(RuntimeError, 'HTTP 404'):
                preflight.check('fixture-user', 'fixture-secret')
            self.assertEqual(request.call_count, 2)
        result, calls = self.run_check([AUTH, (404, None), (201, PUBLIC), (200, PUBLIC)], True)
        self.assertTrue(result['created'])
        self.assertEqual(calls[2].args[:3], ('POST', preflight.REPOSITORIES, 'fixture-bearer'))
        self.assertEqual({key: calls[2].args[3][key] for key in ['namespace', 'name', 'is_private']}, PUBLIC)

    def test_denials_and_server_failures_never_create(self):
        for status in [401, 403, 429, 500]:
            with self.subTest(status=status), patch.object(preflight, 'request', side_effect=[AUTH, (status, None)]) as request:
                with self.assertRaisesRegex(RuntimeError, f'HTTP {status}'):
                    preflight.check('fixture-user', 'fixture-secret', True)
                self.assertEqual(request.call_count, 2)

    def test_private_or_wrong_repository_never_changes_visibility(self):
        for value in [dict(PUBLIC, is_private=True), dict(PUBLIC, namespace='wrong'), dict(PUBLIC, name='wrong')]:
            with patch.object(preflight, 'request', side_effect=[AUTH, (200, value)]) as request:
                with self.assertRaisesRegex(RuntimeError, 'not changed'):
                    preflight.check('fixture-user', 'fixture-secret', True)
                self.assertEqual(request.call_count, 2)

    def test_failed_creation_is_not_retried(self):
        with patch.object(preflight, 'request', side_effect=[AUTH, (404, None), (500, None)]) as request:
            with self.assertRaisesRegex(RuntimeError, 'inspect the repository'):
                preflight.check('fixture-user', 'fixture-secret', True)
            self.assertEqual(request.call_count, 3)

    def test_auth_error_does_not_disclose_response(self):
        with patch.object(preflight, 'request', return_value=(401, {'message': 'fixture-secret'})):
            with self.assertRaisesRegex(RuntimeError, 'authentication failed') as caught:
                preflight.check('fixture-user', 'fixture-secret', True)
            self.assertNotIn('fixture-secret', str(caught.exception))

    @unittest.skipIf(os.name == 'nt', 'This workflow shell runs only on Ubuntu')
    def test_workflow_pipeline_preserves_preflight_failure(self):
        workflow = Path(__file__).resolve().parents[2] / '.github/workflows/release-registry.yml'
        body = workflow.read_text().split('        run: |\n', 1)[1].split('      - uses:', 1)[0]
        body = '\n'.join(line[10:] for line in body.splitlines())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = root / 'scripts/release/registry_preflight.py'
            script.parent.mkdir(parents=True)
            script.write_text('raise SystemExit(7)\n')
            result = subprocess.run(['bash', '-e', '-c', body], cwd=root,
                                    env=dict(os.environ, CREATE_IF_MISSING='false'), capture_output=True)
            self.assertEqual(result.returncode, 7)

    def test_untrusted_event_cannot_authenticate(self):
        for changed in [{'GITHUB_REF': 'refs/heads/other'}, {'GITHUB_REPOSITORY': 'other/repo'},
                        {'GITHUB_EVENT_NAME': 'pull_request'}]:
            environment = dict(GITHUB_REF='refs/heads/main', GITHUB_REPOSITORY='opensourceops/agentctl',
                               GITHUB_EVENT_NAME='workflow_dispatch')
            environment.update(changed)
            with patch.dict(os.environ, environment), patch('sys.argv', ['registry_preflight.py']), patch.object(preflight, 'request') as request:
                with self.assertRaisesRegex(SystemExit, 'trusted manual workflow'):
                    preflight.main()
                request.assert_not_called()

    def test_malformed_auth_never_checks_or_creates_repository(self):
        for auth in [None, {}, {'access_token': 1}, {'access_token': ''}]:
            with patch.object(preflight, 'request', return_value=(200, auth)) as request:
                with self.assertRaisesRegex(RuntimeError, 'authentication failed'):
                    preflight.check('fixture-user', 'fixture-secret', True)
                self.assertEqual(request.call_count, 1)

    def test_failed_anonymous_verification_is_not_success(self):
        with self.assertRaisesRegex(RuntimeError, 'Public repository verification'):
            self.run_check([AUTH, (200, PUBLIC), (401, None)])

    def test_transport_timeout_does_not_retry_or_print_secret(self):
        with patch.object(preflight.urllib.request, 'build_opener') as opener:
            opener.return_value.open.side_effect = TimeoutError('fixture-secret')
            with self.assertRaisesRegex(RuntimeError, 'inspect remote state') as caught:
                preflight.request('POST', preflight.REPOSITORIES, 'fixture-secret', PUBLIC)
            opener.return_value.open.assert_called_once()
            self.assertNotIn('fixture-secret', str(caught.exception))


if __name__ == '__main__':
    unittest.main()
