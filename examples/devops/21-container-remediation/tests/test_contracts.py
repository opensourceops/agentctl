"""Credential-free packaging, execution-boundary and remote reconciliation contracts."""
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile
sys.path.insert(0, str(Path(__file__).resolve().parents[1]/'remediation'))
import adapter
import export
import publisher
import runner
import reconcile_guard
from yaml_io import load

ROOT = Path(__file__).resolve().parents[1]


class ContractTests(unittest.TestCase):
    def test_archive_is_repeatable_and_every_manifest_name_resolves(self):
        sha = runner.git('rev-parse', 'HEAD', cwd=ROOT)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first = export.export(root/'one', sha, True, root/'one.zip')
            export.export(root/'two', sha, True, root/'two.zip')
            self.assertEqual((root/'one.zip').read_bytes(), (root/'two.zip').read_bytes())
            with zipfile.ZipFile(root/'one.zip') as archive:
                manifest = json.loads(archive.read('21-container-remediation/package-manifest.json'))
                for name, digest in manifest['files'].items():
                    self.assertNotIn('\\', name)
                    self.assertEqual(hashlib.sha256(archive.read('21-container-remediation/'+name)).hexdigest(), digest)
            export.verify(root/'one', True)
            if first['sourceDirty']:
                with self.assertRaises(ValueError): export.verify(root/'one')
            with self.assertRaises(ValueError): export.export(root/'one', sha, True)
            (root/'one/requirements.in').write_text('tampered')
            with self.assertRaises(ValueError): export.verify(root/'one', True)

    def test_archive_escape_and_symlink_are_rejected(self):
        import io, tarfile
        for name, kind in [('../escape', tarfile.REGTYPE), ('absolute', tarfile.SYMTYPE)]:
            data = io.BytesIO()
            with tarfile.open(fileobj=data, mode='w') as archive:
                item = tarfile.TarInfo(name); item.type = kind
                archive.addfile(item)
            with tempfile.TemporaryDirectory() as temporary:
                with self.assertRaises(ValueError): runner.safe_archive(data.getvalue(), Path(temporary)/'source')

    def test_tooling_selection_requires_content_identity(self):
        for value in ['latest', 'opensourceops/agentctl:ci', 'sha256:not-a-digest']:
            with self.assertRaises(ValueError): runner.immutable_image(value)
        value = 'docker.io/opensourceops/agentctl@sha256:'+'a'*64
        self.assertEqual(runner.immutable_image(value), value)

    def test_agent_mounts_do_not_expose_publisher_or_engine(self):
        with patch.dict(os.environ, {name: 'fixture' for name in runner.BINDINGS} | {'OPENAI_API_KEY': 'fixture-only', 'GH_TOKEN': 'must-not-forward'}, clear=True):
            argv = runner.container_base('docker', Path('/w'), 'sha256:'+'a'*64, online=True, budget=Path('/budget'))
        text = ' '.join(argv)
        self.assertNotIn('GH_TOKEN', text)
        self.assertNotIn('docker.sock', text)
        self.assertNotIn('fixture-only', text)
        self.assertIn('dst=/workspace,readonly', text)
        self.assertIn('dst=/ci-budget', text)
        self.assertIn('--cap-drop=ALL', argv)
        self.assertIn('--read-only', argv)
        self.assertIn('OPENAI_API_KEY', argv)
        offline = runner.container_base('docker', Path('/w'), 'sha256:'+'a'*64)
        self.assertIn('--network=none', offline)
        self.assertNotIn('OPENAI_API_KEY', offline)

    def test_model_handoff_and_tools_have_exact_scope(self):
        workflow = load(ROOT/'agentctl/remediate.yaml')['spec']
        self.assertEqual(len(workflow['agents']), 2)
        self.assertEqual(workflow['agents']['analyzer']['tools'], [])
        self.assertEqual(workflow['agents']['implementer']['tools'], ['write_manifest', 'write_lock'])
        for agent in workflow['agents'].values():
            self.assertEqual(agent['model'], 'gpt-6-astra')
            self.assertEqual(agent['reasoning'], {'effort': 'high'})
        for tool, filename in [('write_manifest', 'requirements.in'), ('write_lock', 'requirements.lock')]:
            schema = workflow['tools'][tool]['inputSchema']['properties']
            self.assertEqual(schema['path']['enum'], ['patch/'+filename])
            self.assertEqual(schema['content']['enum'], [adapter.expected_files('2.7.0')[filename].decode()])
        task = next(t for t in workflow['tasks'] if t['id'] == 'implement')
        self.assertEqual(task['with']['prompt'], '${{ tasks.validate-plan.output }}')
        eligibility = load(ROOT/'agentctl/eligibility.yaml')['spec']
        self.assertFalse(eligibility.get('providers'))
        self.assertFalse(eligibility.get('agents'))

    def test_ci_separates_credentials_and_forbids_untrusted_dispatch(self):
        workflow = load(ROOT/'.github/workflows/remediation.yml')
        self.assertNotIn('pull_request_target', workflow['on'])
        jobs = workflow['jobs']
        self.assertIn("github.ref == 'refs/heads/main'", jobs['remediate']['if'])
        self.assertNotIn('GH_TOKEN', json.dumps(jobs['remediate']))
        self.assertNotIn('OPENAI_API_KEY', json.dumps(jobs['publish']))
        self.assertIn('secrets.GH_TOKEN', json.dumps(jobs['publish']))
        self.assertEqual(workflow['permissions'], {'contents': 'read'})
        self.assertNotIn('OPENAI_API_KEY', json.dumps(jobs['reconcile']))
        self.assertEqual(jobs['reconcile']['permissions']['actions'], 'read')

    def test_pr_creation_lost_acknowledgement_reconciles_without_second_post(self):
        class Remote:
            calls = 0
            created = False
            def pulls(self, branch, base):
                return [{'html_url': 'https://github.com/fixture/demo/pull/1', 'state': 'open'}] if self.created else []
            def request(self, method, path, body):
                self.calls += 1; self.created = True
                raise RuntimeError('fixture lost acknowledgement after remote create')
        remote = Remote()
        result = publisher.reconcile_pull(remote, 'branch', 'main', {})
        self.assertEqual(result['status'], 'reconciled_after_uncertain_create')
        self.assertEqual(remote.calls, 1)
        repeated = publisher.reconcile_pull(remote, 'branch', 'main', {})
        self.assertEqual(repeated['status'], 'reused')
        self.assertEqual(remote.calls, 1)

    def test_unknown_post_outcome_retains_uncertainty_and_closed_pr_is_not_duplicated(self):
        class Remote:
            calls = 0
            def pulls(self, branch, base): return []
            def request(self, method, path, body): self.calls += 1; raise RuntimeError('fixture transport failure')
        remote = Remote()
        with self.assertRaises(RuntimeError): publisher.reconcile_pull(remote, 'branch', 'main', {})
        self.assertEqual(remote.calls, 1)
        remote.pulls = lambda *_: [{'html_url': 'https://github.com/fixture/demo/pull/1', 'state': 'closed'}]
        self.assertEqual(publisher.reconcile_pull(remote, 'branch', 'main', {})['status'], 'closed_no_duplicate')
        self.assertEqual(remote.calls, 1)

    def test_publisher_git_cannot_fall_back_to_inherited_credentials(self):
        result = publisher.git_environment({'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'credential.helper',
                                            'GIT_CONFIG_VALUE_0': 'privileged-helper', 'GIT_ASKPASS': '/bad/helper',
                                            'SSH_ASKPASS': '/bad/ssh', 'GH_TOKEN': 'scoped-fixture-only'})
        self.assertNotIn('GIT_CONFIG_COUNT', result)
        self.assertNotIn('GIT_CONFIG_KEY_0', result)
        self.assertNotIn('GIT_ASKPASS', result)
        self.assertNotIn('SSH_ASKPASS', result)
        self.assertEqual(result['GIT_CONFIG_GLOBAL'], '/dev/null')
        self.assertEqual(result['GIT_CONFIG_NOSYSTEM'], '1')
        self.assertEqual(result['GH_TOKEN'], 'scoped-fixture-only')

    def test_restore_requires_exact_successful_validation_job_and_source(self):
        policy = adapter.contract()
        class Remote:
            run = {'repository': {'full_name': policy['repository']}, 'head_sha': 'a'*40, 'path': '.github/workflows/remediation.yml',
                   'head_branch': 'main', 'event': 'workflow_dispatch', 'status': 'completed', 'run_attempt': 1, 'conclusion': 'failure'}
            jobs = {'total_count': 2, 'jobs': [{'name': 'remediate', 'conclusion': 'success'}, {'name': 'publish', 'conclusion': 'failure'}]}
            artifacts = {'total_count': 1, 'artifacts': [{'id': 99, 'name': 'publication-42-1', 'expired': False,
                                                        'workflow_run': {'head_sha': 'a'*40}, 'digest': 'sha256:'+'b'*64}]}
            def request(self, method, path):
                if '/jobs?' in path: return self.jobs
                if '/artifacts?' in path: return self.artifacts
                return self.run
        remote = Remote()
        result = reconcile_guard.verify_run(remote, '42', '1', 'a'*40)
        self.assertEqual(result['artifactId'], 99)
        self.assertEqual(result['priorWorkflowConclusion'], 'failure')
        for field, value in [('head_sha', 'c'*40), ('path', '.github/workflows/untrusted.yml'), ('status', 'in_progress')]:
            original = remote.run; remote.run = {**original, field: value}
            with self.assertRaises(ValueError): reconcile_guard.verify_run(remote, '42', '1', 'a'*40)
            remote.run = original
        remote.jobs = {'total_count': 1, 'jobs': [{'name': 'remediate', 'conclusion': 'failure'}]}
        with self.assertRaises(ValueError): reconcile_guard.verify_run(remote, '42', '1', 'a'*40)

    def test_publisher_rejects_wrong_repository_and_branch_namespace(self):
        with self.assertRaises(ValueError): publisher.GitHub('opensourceops/agentctl', 'fixture-only')
        with self.assertRaises(ValueError): publisher.checked_branch({'fingerprint': 'a'*64}, {'branchPrefix': 'main/'})
        self.assertEqual(publisher.checked_branch({'fingerprint': 'a'*64}, adapter.contract()), 'agentctl/remediation/'+'a'*24)

if __name__ == '__main__': unittest.main()
