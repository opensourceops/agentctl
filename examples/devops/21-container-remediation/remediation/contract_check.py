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
import preflight
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


def run_id(envelope):
    identifier = envelope.get('data', {}).get('runId') or envelope.get('error', {}).get('runId')
    if not isinstance(identifier, str) or not identifier:
        raise ValueError('CLI result did not identify its durable run')
    return identifier


def passing_gates(workspace, metadata):
    after = {'SchemaVersion': 2, 'ArtifactType': 'container_image', 'Metadata': {'ImageID': 'sha256:'+'f'*64}, 'Results': [
        {'Target': 'SYNTHETIC/site-packages', 'Class': 'lang-pkgs', 'Type': 'python-pkg', 'Packages': [{'Name': 'urllib3', 'Version': '2.7.0'}], 'Vulnerabilities': []}]}
    runner.save(workspace/'inputs/after.json', after)
    gate = {**{key: metadata[key] for key in ['sourceSha', 'sourceTree', 'databaseSha256', 'databaseMetadataSha256', 'scannerImage', 'reportSha256']},
            'testsPassed': True, 'buildSucceeded': True, 'scanSucceeded': True, 'patchDigest': runner.json_file(workspace/'state/patch-ready.json')['patchDigest'],
            'validatedTree': '1'*40, 'beforeImageId': metadata['imageId'], 'afterImageId': 'sha256:'+'f'*64, 'afterReportSha256': runner.digest_file(workspace/'inputs/after.json')}
    runner.save(workspace/'inputs/gates.json', gate)
    return gate


def recovery_fixture(engine, image, out):
    workspace, value, metadata = fixture(out)
    spec = value['spec']
    spec['actions']['analysis-checkpoint'] = {'kind': 'builtin.assert'}
    index = next(i for i, task in enumerate(spec['tasks']) if task['id'] == 'analyze') + 1
    checkpoint = {'id': 'analysis-checkpoint', 'uses': 'action:analysis-checkpoint', 'needs': ['analyze'],
                  'with': {'that': False, 'message': 'SYNTHETIC terminal failure after completed analysis'}}
    spec['tasks'].insert(index, checkpoint)
    next(task for task in spec['tasks'] if task['id'] == 'validate-plan')['needs'].append('analysis-checkpoint')
    dump(workspace/'failed-after-analysis.yaml', value)
    database = 'state/recovery.sqlite3'
    source = command(engine, image, workspace, ['run', 'failed-after-analysis.yaml', '--workspace', '/workspace', '--db', database],
                     'failed-after-analysis', expected=4)
    source_id = run_id(source)
    original = command(engine, image, workspace, ['inspect', source_id, '--db', database], 'failed-inspect')['data']
    tasks = {task['taskId']: task for task in original['tasks']}
    if (original['run']['state'] != 'failed' or tasks['analyze']['state'] != 'succeeded'
            or tasks['analysis-checkpoint']['state'] != 'failed' or original['budget']['usage']['providerRequests'] != 1):
        raise ValueError('failure injection did not stop immediately after successful analysis')
    if original['toolCalls'] or list((workspace/'patch').iterdir()) or any((workspace/'state'/name).exists() for name in ['patch-ready.json', 'publication.json']):
        raise ValueError('failure checkpoint allowed downstream mutation or publication')

    # Only this explicit deterministic checkpoint changes. The reviewed policy,
    # captured inputs, instructions, analyzer and its typed handoff stay identical.
    checkpoint['with']['that'] = True
    dump(workspace/'repaired.yaml', value)
    repair_args = ['repair', 'repaired.yaml', source_id, '--from', 'analysis-checkpoint',
                   '--reason', 'Repair the injected post-analysis fixture failure', '--workspace', '/workspace', '--db', database]
    preview = command(engine, image, workspace, repair_args + ['--plan'], 'repair-plan')['data']
    if not preview['compatible'] or set(preview['reusedTasks']) != {'capture', 'analyze'} or preview['blockedReuse']:
        raise ValueError('repair preview did not preserve exactly the compatible completed boundaries')
    repaired = command(engine, image, workspace, repair_args, 'repair')
    repaired_id = run_id(repaired)
    inspection = command(engine, image, workspace, ['inspect', repaired_id, '--db', database], 'repair-inspect')['data']
    repaired_tasks = {task['taskId']: task for task in inspection['tasks']}
    analyzer = repaired_tasks['analyze']
    if (inspection['run']['state'] != 'succeeded' or analyzer['disposition'] != 'reused'
            or {task['taskId'] for task in inspection['tasks'] if task['disposition'] == 'reused'} != {'capture', 'analyze'}
            or analyzer['sourceRunId'] != source_id or analyzer['sourceTaskId'] != 'analyze'
            or analyzer['output'] != tasks['analyze']['output'] or analyzer['attempt'] != 0
            or inspection['budget']['usage']['providerRequests'] != 4 or len(inspection['toolCalls']) != 2
            or any(effect['request']['taskId'] in {'capture', 'analyze'} for effect in inspection['effects'])):
        raise ValueError('selective repair reran analysis or failed to execute the two scoped mutation stages')
    if {name: (workspace/'patch'/name).read_bytes() for name in adapter.contract()['allowedFiles']} != adapter.expected_files('2.7.0'):
        raise ValueError('repaired run did not produce the exact validated patch')
    replay = command(engine, image, workspace, ['replay', repaired_id, '--db', database], 'repair-replay')
    replay_id = run_id(replay)
    replay_inspection = command(engine, image, workspace, ['inspect', replay_id, '--db', database], 'repair-replay-inspect')['data']
    if replay_inspection['effects'] or replay_inspection['budget']['usage']['providerRequests'] != 0:
        raise ValueError('repaired-run replay introduced fresh effects')

    # No successful eligibility run precedes these cases: any publication file
    # would have been produced by the rejected validation attempt itself.
    gates = passing_gates(workspace, metadata)
    failed_validations = []
    for flag in ['testsPassed', 'buildSucceeded', 'scanSucceeded']:
        failed_gates = {**gates, flag: False}
        captured_gate = workspace/'inputs'/('failed-'+flag+'-gates.json')
        runner.save(captured_gate, failed_gates)
        runner.save(workspace/'inputs/gates.json', failed_gates)
        failed_db = 'state/failed-'+flag+'.sqlite3'
        failure = command(engine, image, workspace, ['run', 'eligibility.yaml', '--workspace', '/workspace', '--db', failed_db],
                          'failed-'+flag, expected=4)
        failure_id = run_id(failure)
        failed = command(engine, image, workspace, ['inspect', failure_id, '--db', failed_db], 'inspect-failed-'+flag)['data']
        if (failed['run']['state'] != 'failed' or failed['budget']['usage']['providerRequests'] != 0
                or (workspace/'state/publication.json').exists()):
            raise ValueError('failed trusted validation produced publication eligibility')
        failed_validations.append({'gate': flag, 'runId': failure_id, 'exitCode': 4, 'publicationFileExists': False,
                                   'gateSha256': runner.digest_file(captured_gate),
                                   'effectStatuses': [effect['status'] for effect in failed['effects']]})
    runner.save(workspace/'inputs/gates.json', gates)
    control = command(engine, image, workspace, ['run', 'eligibility.yaml', '--workspace', '/workspace', '--db', 'state/passing-control.sqlite3'],
                      'passing-validation-control')
    if runner.json_file(workspace/'state/publication.json')['eligible'] is not True:
        raise ValueError('restoring only the three passing gate flags did not restore publication eligibility')
    return {'evidenceKind': 'injected terminal failure after analysis, then compatible selective repair; no process-crash or same-run resume claim',
            'sourceRunId': source_id, 'repairRunId': repaired_id, 'replayRunId': replay_id,
            'reusedTasks': preview['reusedTasks'], 'sourceProviderRequests': 1, 'repairProviderRequests': 4,
            'freshAnalyzerEffects': 0, 'replayFreshEffects': 0, 'failedValidations': failed_validations,
            'passingValidationControlRunId': run_id(control)}



def preflight_fixture(engine, image, out):
    workspace = out/'workspace'
    for name in ['state', 'patch', 'inputs']:
        (workspace/name).mkdir(parents=True)
    shutil.copytree(ROOT, workspace/'source', ignore=shutil.ignore_patterns('__pycache__', '*.pyc', '.git', '.venv', 'state'))
    value = preflight.configure(workspace)
    for operation in ['check', 'plan']:
        command(engine, image, workspace, [operation, 'preflight.yaml', '--workspace', '/workspace'], 'authored-'+operation)
    value['metadata']['name'] = 'synthetic-responses-tool-preflight'
    value['spec']['providers'] = {'fake': {'kind': 'fake'}}
    agent = value['spec']['agents']['probe']
    agent.update({'provider': 'fake', 'model': 'scripted', 'providerOptions': {'finalText': '{"echo":"ok"}', 'toolInput': {'text': 'ok'}}})
    agent.pop('reasoning')
    prices = value['spec']['runtime']['pricing']['models']
    prices['fake/scripted'] = prices.pop('openai/gpt-6-astra')
    dump(workspace/'preflight.yaml', value)
    result = command(engine, image, workspace, ['run', 'preflight.yaml', '--workspace', '/workspace', '--db', 'state/preflight.sqlite3'], 'scripted-run')
    inspection = runner.inspect_replay(engine, workspace, image, result, 'state/preflight.sqlite3', 'preflight')
    preflight.assert_compatibility(inspection)
    return {'runId': result['data']['runId'], 'toolCalls': 1, 'replayFreshEffects': 0, 'providerNetworkRequests': 0,
            'evidenceKind': 'scripted fake provider; real pure echo tool and strict output'}


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
    passing_gates(workspace, metadata)
    eligible = command(args.engine, args.tooling_image, workspace, ['run', 'eligibility.yaml', '--workspace', '/workspace', '--db', 'state/eligibility.sqlite3'], 'eligibility')
    if not runner.json_file(workspace/'state/publication.json')['eligible']: raise ValueError('synthetic passing gates were rejected')
    denied, deny_value, _ = fixture(out/'denied')
    deny_value['spec']['policy']['toolsDeny'] = ['write_manifest', 'write_lock']
    dump(denied/'contract.yaml', deny_value)
    failure = command(args.engine, args.tooling_image, denied, ['run', 'contract.yaml', '--workspace', '/workspace', '--db', 'state/run.sqlite3'], 'denied-run', expected=4)
    if list((denied/'patch').iterdir()) or (denied/'state/patch-ready.json').exists():
        raise ValueError('denied writes produced forbidden side effects')
    recovery = recovery_fixture(args.engine, args.tooling_image, out/'recovery')
    preflight_result = preflight_fixture(args.engine, args.tooling_image, out/'preflight')
    runner.save(out/'report.json', {'preflight': preflight_result, 'recovery': recovery,
                'evidenceKind': 'synthetic scanner and scripted fake provider; real CLI and OCI',
                'toolingImage': args.tooling_image, 'runId': identifier, 'replayRunId': replay_id,
                'eligibilityRunId': eligible['data']['runId'], 'denialRunId': run_id(failure),
                'toolCalls': 2, 'replayFreshEffects': 0, 'providerNetworkRequests': 0, 'status': 'passed'})
    print((out/'report.json').read_text())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', default='docker'); parser.add_argument('--tooling-image', required=True)
    parser.add_argument('--out', type=Path, required=True)
    run(parser.parse_args())

if __name__ == '__main__': main()
