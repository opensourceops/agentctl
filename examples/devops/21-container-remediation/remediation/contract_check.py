#!/usr/bin/env python3
"""Real CLI/OCI checks with explicitly SYNTHETIC scanner/model fixtures; no paid calls."""
import argparse
import copy
import json
from pathlib import Path
import shutil
import sys
import adapter
import runner
from yaml_io import load, dump

ROOT = Path(__file__).resolve().parent.parent


def fixture(out):
    workspace = out/'workspace'
    workspace.mkdir(parents=True)
    shutil.copytree(ROOT, workspace/'source', ignore=shutil.ignore_patterns('__pycache__', '*.pyc', '.git', '.venv', 'state'))
    for name in ['state', 'patch', 'inputs']:
        (workspace/name).mkdir()
    before = {'SchemaVersion': 2, 'ArtifactType': 'container_image', 'Metadata': {'ImageID': 'sha256:'+'a'*64}, 'Results': [
        {'Target': 'SYNTHETIC/site-packages', 'Class': 'lang-pkgs', 'Type': 'python-pkg', 'Packages': [{'Name': 'urllib3', 'Version': '2.6.2'}],
         'Vulnerabilities': [{'VulnerabilityID': 'CVE-FIXTURE-ONLY', 'PkgName': 'urllib3', 'InstalledVersion': '2.6.2', 'FixedVersion': '2.7.0', 'Severity': 'HIGH'}]}]}
    runner.save(workspace/'inputs/before.json', before)
    metadata = {'repository': adapter.contract()['repository'], 'baseBranch': 'main', 'sourceSha': 'b'*40, 'sourceTree': 'c'*40,
                'imageId': 'sha256:'+'a'*64, 'reportSha256': runner.digest_file(workspace/'inputs/before.json'), 'databaseSha256': 'd'*64,
                'databaseMetadataSha256': 'e'*64, 'scannerImage': adapter.contract()['scannerImage']}
    runner.save(workspace/'inputs/scan-context.json', metadata)
    adapter.ROOT = workspace.resolve()
    context = adapter.capture({})
    plan = {'decision': 'remediate', **{name: context[name] for name in ['sourceSha', 'reportSha256', 'package', 'installedVersion', 'targetVersion', 'files']},
            'advisories': [{name: row[name] for name in ['id', 'evidence', 'fixedVersions']} for row in context['targets']],
            'compatibility': 'SYNTHETIC fixture; no actual model or scan evidence.', 'manualItems': []}
    runner.configure(workspace)
    value = load(workspace/'remediate.yaml')
    spec = value['spec']; value['metadata']['name'] = 'synthetic-container-remediation-contract'
    spec['providers'] = {'fake': {'kind': 'fake'}}
    spec['runtime'].pop('pricing')
    spec['runtime']['budgets'].pop('maxCostMicrousd')
    spec['runtime']['budgets'].update({'maxProviderRequests': 5, 'maxTurns': 5})
    implementation = {'applied': True, 'package': 'urllib3', 'targetVersion': '2.7.0', 'files': context['files']}
    for agent in spec['agents'].values():
        agent['provider'] = 'fake'; agent['model'] = 'scripted'; agent.pop('reasoning'); agent['providerOptions'] = {}
    spec['agents']['analyzer']['providerOptions'] = {'finalText': json.dumps(plan)}
    # Existing fake provider issues one first-tool call per agent. The contract
    # fixture deliberately splits B's two writes into two scripted stages. The
    # live authored workflow keeps one implementer agent with both scoped tools.
    spec['agents']['manifest-fixture'] = copy.deepcopy(spec['agents']['implementer'])
    for name, tool, filename in [('manifest-fixture', 'write_manifest', 'requirements.in'), ('implementer', 'write_lock', 'requirements.lock')]:
        spec['agents'][name]['tools'] = [tool]
        spec['agents'][name]['maxToolCalls'] = 1
        spec['agents'][name]['providerOptions'] = {'finalText': json.dumps(implementation), 'toolInput': {
            'path': 'patch/'+filename, 'content': adapter.expected_files('2.7.0')[filename].decode()}}
    index = next(i for i, task in enumerate(spec['tasks']) if task['id'] == 'implement')
    stage = copy.deepcopy(spec['tasks'][index]); stage['id'] = 'manifest-fixture'; stage['uses'] = 'agent:manifest-fixture'
    spec['tasks'].insert(index, stage)
    spec['tasks'][index+1]['needs'].append('manifest-fixture')
    dump(workspace/'contract.yaml', value)
    return workspace, value, metadata


def command(engine, image, workspace, args, label, expected=0):
    result = runner.execute(runner.container_base(engine, workspace, image)+['--output', 'json', *args],
                            log=workspace.parent/'logs'/label, allowed=(expected,))
    envelopes = []
    for raw in [result.stdout, result.stderr]:
        for line in raw.splitlines():
            try:
                value = json.loads(line)
                if value.get('apiVersion') == 'agentctl.dev/cli/v1': envelopes.append(value)
            except (ValueError, AttributeError): pass
    if len(envelopes) != 1: raise ValueError('expected one stable CLI envelope')
    return envelopes[0]


def run(args):
    out = args.out.resolve(); out.mkdir(parents=True, exist_ok=False)
    workspace, value, metadata = fixture(out/'positive')
    for sub in ['check', 'plan']:
        command(args.engine, args.tooling_image, workspace, [sub, 'contract.yaml', '--workspace', '/workspace'], sub)
    result = command(args.engine, args.tooling_image, workspace, ['run', 'contract.yaml', '--workspace', '/workspace', '--db', 'state/run.sqlite3'], 'run')
    identifier = result['data']['runId']
    inspection = command(args.engine, args.tooling_image, workspace, ['inspect', identifier, '--db', 'state/run.sqlite3'], 'inspect')['data']
    if inspection['run']['state'] != 'succeeded' or len(inspection['toolCalls']) != 2:
        raise ValueError('scripted contract did not execute both real scoped write tools')
    if {name: (workspace/'patch'/name).read_bytes() for name in adapter.contract()['allowedFiles']} != adapter.expected_files('2.7.0'):
        raise ValueError('actual tool writes did not produce reviewed bytes')
    replay = command(args.engine, args.tooling_image, workspace, ['replay', identifier, '--db', 'state/run.sqlite3'], 'replay')
    replay_id = replay['data']['runId']
    replay_inspection = command(args.engine, args.tooling_image, workspace, ['inspect', replay_id, '--db', 'state/run.sqlite3'], 'replay-inspect')['data']
    if replay_inspection['effects'] or replay_inspection['budget']['usage']['providerRequests'] != 0:
        raise ValueError('networkless keyless replay produced fresh effects or provider requests')
    after = {'SchemaVersion': 2, 'ArtifactType': 'container_image', 'Metadata': {'ImageID': 'sha256:'+'f'*64}, 'Results': [
        {'Target': 'SYNTHETIC/site-packages', 'Class': 'lang-pkgs', 'Type': 'python-pkg', 'Packages': [{'Name': 'urllib3', 'Version': '2.7.0'}], 'Vulnerabilities': []}]}
    runner.save(workspace/'inputs/after.json', after)
    gate = {**{key: metadata[key] for key in ['sourceSha', 'sourceTree', 'databaseSha256', 'databaseMetadataSha256', 'scannerImage', 'reportSha256']},
            'testsPassed': True, 'buildSucceeded': True, 'scanSucceeded': True, 'patchDigest': runner.json_file(workspace/'state/patch-ready.json')['patchDigest'],
            'validatedTree': '1'*40, 'beforeImageId': metadata['imageId'], 'afterImageId': 'sha256:'+'f'*64, 'afterReportSha256': runner.digest_file(workspace/'inputs/after.json')}
    runner.save(workspace/'inputs/gates.json', gate)
    eligible = command(args.engine, args.tooling_image, workspace, ['run', 'eligibility.yaml', '--workspace', '/workspace', '--db', 'state/eligibility.sqlite3'], 'eligibility')
    if not runner.json_file(workspace/'state/publication.json')['eligible']: raise ValueError('synthetic passing gates were rejected')
    denied, deny_value, _ = fixture(out/'denied')
    deny_value['spec']['policy']['toolsDeny'] = ['write_manifest', 'write_lock']
    dump(denied/'contract.yaml', deny_value)
    failure = command(args.engine, args.tooling_image, denied, ['run', 'contract.yaml', '--workspace', '/workspace', '--db', 'state/run.sqlite3'], 'denied-run', expected=4)
    if list((denied/'patch').iterdir()) or (denied/'state/patch-ready.json').exists():
        raise ValueError('denied writes produced forbidden side effects')
    runner.save(out/'report.json', {'evidenceKind': 'synthetic scanner and scripted fake provider; real CLI and OCI',
                'toolingImage': args.tooling_image, 'runId': identifier, 'replayRunId': replay_id,
                'eligibilityRunId': eligible['data']['runId'], 'denialRunId': failure.get('error', {}).get('runId'),
                'toolCalls': 2, 'replayFreshEffects': 0, 'providerNetworkRequests': 0, 'status': 'passed'})
    print((out/'report.json').read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', default='docker'); parser.add_argument('--tooling-image', required=True)
    parser.add_argument('--out', type=Path, required=True)
    run(parser.parse_args())

if __name__ == '__main__': main()
