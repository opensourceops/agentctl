"""Credential-free packaging, execution-boundary and remote reconciliation contracts."""
import copy
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
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
import preflight
import build_workflows
import contract_check
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

    def test_already_exported_git_root_reexports_without_hashing_its_own_manifest(self):
        framework_sha = runner.git('rev-parse', 'HEAD', cwd=ROOT)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            standalone = root/'standalone'
            export.export(standalone, framework_sha, True)
            original_manifest = (standalone/'package-manifest.json').read_bytes()
            # This is an ordinary payload with the same basename, not the root
            # exporter's own manifest. Its bytes must remain in the inventory.
            (standalone/'metadata').mkdir()
            (standalone/'metadata/package-manifest.json').write_text('{"fixture":"nested payload"}\n')
            runner.git('init', '--initial-branch=main', cwd=standalone)
            runner.git('add', '.', cwd=standalone)
            runner.git('-c', 'user.name=Export fixture', '-c', 'user.email=fixture@example.invalid',
                       'commit', '-m', 'Local standalone export fixture', cwd=standalone)
            # Explicitly qualified previews keep the real framework identity;
            # this local fixture's Git commit is not a framework release SHA.
            with patch.object(export, 'SOURCE', standalone):
                first = export.export(root/'again', framework_sha, True, root/'again.zip')
                export.export(root/'repeat', framework_sha, True, root/'repeat.zip')
            self.assertEqual((standalone/'package-manifest.json').read_bytes(), original_manifest)
            self.assertNotIn('package-manifest.json', first['files'])
            self.assertIn('metadata/package-manifest.json', first['files'])
            self.assertEqual((root/'again.zip').read_bytes(), (root/'repeat.zip').read_bytes())
            with zipfile.ZipFile(root/'again.zip') as archive:
                manifest = json.loads(archive.read('21-container-remediation/package-manifest.json'))
                for name, digest in manifest['files'].items():
                    self.assertEqual(hashlib.sha256(archive.read('21-container-remediation/'+name)).hexdigest(), digest)
            export.verify(root/'again', True)

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
        # This unit constructs Linux OCI arguments even on a Windows test host.
        with patch.object(os, 'getuid', return_value=1000, create=True), patch.object(os, 'getgid', return_value=1000, create=True), \
             patch.dict(os.environ, {name: 'fixture' for name in runner.BINDINGS} | {'OPENAI_API_KEY': 'fixture-only', 'GH_TOKEN': 'must-not-forward'}, clear=True):
            argv = runner.container_base('docker', PurePosixPath('/w'), 'sha256:'+'a'*64, online=True, budget=PurePosixPath('/budget'))
            offline = runner.container_base('docker', PurePosixPath('/w'), 'sha256:'+'a'*64)
        text = ' '.join(argv)
        self.assertNotIn('GH_TOKEN', text)
        self.assertNotIn('docker.sock', text)
        self.assertNotIn('fixture-only', text)
        self.assertIn('dst=/workspace,readonly', text)
        self.assertIn('dst=/ci-budget', text)
        self.assertIn('--cap-drop=ALL', argv)
        self.assertIn('--read-only', argv)
        self.assertIn('OPENAI_API_KEY', argv)
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
            self.assertEqual(set(schema['content']), {'type', 'pattern', 'minLength', 'maxLength'})
            self.assertEqual(schema['content']['type'], 'string')
            pattern = schema['content']['pattern']
            expected = adapter.expected_files('2.7.0')[filename]
            self.assertTrue(expected.isascii())
            self.assertEqual(schema['content']['minLength'], len(expected.decode()))
            self.assertEqual(schema['content']['maxLength'], len(expected.decode()))
            self.assertEqual(workflow['tools'][tool], build_workflows.write_tool(filename, expected))
            self.assertTrue(pattern.startswith('^') and pattern.endswith('$'))
            self.assertNotIn('(?', pattern)
            # Fullmatch models the reference exact-content language, not Python
            # search semantics. The real CLI's 19 rejection cases prove the
            # pinned Rust validator's complete schema boundary before any write.
            self.assertIsNotNone(re.fullmatch(pattern, expected.decode()))
            self.assertNotIn('\n', pattern)
            for label, changed in contract_check.invalid_write_inputs(filename):
                with self.subTest(tool=tool, mutation=label):
                    accepted = (changed['path'] in schema['path']['enum']
                                and schema['content']['minLength'] <= len(changed['content']) <= schema['content']['maxLength']
                                and re.fullmatch(pattern, changed['content']) is not None)
                    self.assertFalse(accepted)
            if filename == 'requirements.lock':
                # Both actual preflights returned this 65-character prefix.
                # The explicit lower bound rejects it independently of pattern.
                shortened = '# Hash-locked pure Python wheel; application dependencies only.\nu'
                self.assertTrue(expected.decode().startswith(shortened))
                self.assertLess(len(shortened), schema['content']['minLength'])
        task = next(t for t in workflow['tasks'] if t['id'] == 'implement')
        self.assertEqual(task['with']['prompt'], '${{ tasks.validate-plan.output }}')
        eligibility = load(ROOT/'agentctl/eligibility.yaml')['spec']
        self.assertFalse(eligibility.get('providers'))
        self.assertFalse(eligibility.get('agents'))

    def test_write_pattern_treats_regex_metacharacters_as_literal_bytes(self):
        content = b'\\^$.*+?()[]{}| -#\t\r\n'
        schema = build_workflows.write_tool('fixture', content)['inputSchema']['properties']['content']
        pattern = schema['pattern']
        self.assertEqual((schema['minLength'], schema['maxLength']), (len(content.decode()), len(content.decode())))
        self.assertIsNotNone(re.fullmatch(pattern, content.decode()))
        for changed in [content[:-1], b'x'+content, content+b'\n', content.replace(b'.', b'x')]:
            self.assertIsNone(re.fullmatch(pattern, changed.decode()))

    def test_both_live_workflows_account_for_cache_reads_and_writes(self):
        for name in ['remediate', 'preflight']:
            with self.subTest(workflow=name):
                prices = load(ROOT/f'agentctl/{name}.yaml')['spec']['runtime']['pricing']
                self.assertEqual(prices['version'], 'openai-public-2026-09-08-estimate')
                self.assertEqual(prices['models']['openai/gpt-6-astra'], {
                    'inputMicrousdPerMillionTokens': 10000000, 'outputMicrousdPerMillionTokens': 50000000,
                    'cacheReadMicrousdPerMillionTokens': 1000000, 'cacheWriteMicrousdPerMillionTokens': 12500000})

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

    def test_preflight_has_its_own_tiny_lease_boundary(self):
        workflow = load(ROOT/'agentctl/preflight.yaml')['spec']
        budget = workflow['runtime']['budgets']
        self.assertEqual((budget['maxProviderRequests'], budget['maxTotalTokens'], budget['maxCostMicrousd']), (3, 8000, 200000))
        self.assertEqual(workflow['agents']['probe']['model'], 'gpt-6-astra')
        self.assertEqual(workflow['agents']['probe']['reasoning'], {'effort': 'high'})
        self.assertEqual(workflow['agents']['probe']['maxToolCalls'], 1)
        self.assertEqual(workflow['agents']['probe']['maxOutputTokens'], 2048)
        self.assertEqual(workflow['tools']['echo']['effectClass'], 'pure')
        self.assertEqual(workflow['tools']['echo']['capability'], 'internal')
        self.assertEqual(workflow['tools']['echo']['kind'], 'builtin.echo')
        self.assertEqual(workflow['policy']['writableRoots'], ['state'])
        self.assertFalse(workflow['policy'].get('processAllowlist'))
        job = load(ROOT/'.github/workflows/remediation.yml')['jobs']['preflight']
        self.assertIn("inputs.operation == 'preflight'", job['if'])
        self.assertNotIn('GH_TOKEN', json.dumps(job))
        self.assertNotIn('runner.py prepare', json.dumps(job))
        self.assertNotIn('publisher.py', json.dumps(job))

    def test_preflight_echo_exercises_both_actual_multiline_write_input_schemas(self):
        preflight_spec = load(ROOT/'agentctl/preflight.yaml')['spec']
        remediation = load(ROOT/'agentctl/remediate.yaml')['spec']
        echo = preflight_spec['tools']['echo']
        self.assertEqual(echo['inputSchema'], echo['outputSchema'])
        self.assertEqual(set(echo['inputSchema']['required']), {'manifest', 'lock'})
        self.assertIs(echo['inputSchema']['additionalProperties'], False)
        for name, tool in [('manifest', 'write_manifest'), ('lock', 'write_lock')]:
            self.assertEqual(echo['inputSchema']['properties'][name], remediation['tools'][tool]['inputSchema'])
        prompt = preflight_spec['tasks'][0]['with']['prompt']
        self.assertEqual(prompt, preflight.expected_echo_input())

    def test_preflight_checks_actual_tool_result_and_strict_output(self):
        good = {'run': {'state': 'succeeded', 'output': {'preflight': {'echo': 'ok'}}},
                'toolCalls': [{'toolId': 'echo', 'status': 'succeeded', 'effectId': 'fixture-effect'}],
                'effects': [{'request': {'id': 'fixture-effect', 'input': preflight.expected_echo_input()},
                             'result': preflight.expected_echo_input(), 'status': 'succeeded'}],
                'budget': {'usage': {'providerRequests': 2, 'inputTokens': 100, 'outputTokens': 50, 'costMicrousd': 3500}}}
        preflight.assert_compatibility(good)
        for field, value in [('toolCalls', []), ('effects', []), ('run', {'state': 'succeeded', 'output': {'preflight': {'echo': 'different'}}})]:
            with self.subTest(field=field):
                with self.assertRaises(ValueError): preflight.assert_compatibility({**good, field: value})
        exceeded = copy.deepcopy(good); exceeded['budget']['usage']['inputTokens'] = 8000
        with self.assertRaises(ValueError): preflight.assert_compatibility(exceeded)
        for location in ['input', 'result']:
            for part in ['manifest', 'lock']:
                with self.subTest(location=location, part=part):
                    changed = copy.deepcopy(good)
                    effect = changed['effects'][0]
                    value = effect['request']['input'] if location == 'input' else effect['result']
                    value[part]['content'] += '\n'
                    with self.assertRaises(ValueError): preflight.assert_compatibility(changed)
        old_probe = copy.deepcopy(good)
        old_probe['effects'][0].update({'request': {'id': 'fixture-effect', 'input': {'text': 'ok'}}, 'result': {'text': 'ok'}})
        with self.assertRaises(ValueError): preflight.assert_compatibility(old_probe)

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
