#!/usr/bin/env python3
"""Trusted outer CI driver. Docker, Git and scanners are never exposed as model tools."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import time
import adapter
from yaml_io import load, dump

ROOT = Path(__file__).resolve().parent.parent
BUDGET = '/opt/agentctl/release-budget/scripts/release_live_budget.py'
BINDINGS = ['GITHUB_REPOSITORY', 'GITHUB_SHA', 'GITHUB_WORKFLOW_REF', 'GITHUB_RUN_ID', 'GITHUB_RUN_NUMBER', 'GITHUB_RUN_ATTEMPT', 'GITHUB_JOB']


def digest_file(path):
    with Path(path).open('rb') as handle:
        return hashlib.file_digest(handle, 'sha256').hexdigest()


def json_file(path):
    return adapter.parse(Path(path).read_bytes())


def save(path, value):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_bytes(adapter.canonical(value) + b'\n')


def execute(args, *, cwd=None, env=None, log=None, data=None, timeout=600, allowed=(0,)):
    """Argument arrays only. Credentials stay in environment, never command records."""
    started = time.monotonic()
    result = subprocess.run([str(v) for v in args], cwd=cwd, env=env, input=data, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=timeout, check=False)
    if log:
        log = Path(log)
        log.parent.mkdir(parents=True, exist_ok=True)
        log.with_suffix('.stdout').write_bytes(result.stdout)
        log.with_suffix('.stderr').write_bytes(result.stderr)
        save(log.with_suffix('.command.json'), {'argv': [str(v) for v in args], 'exitCode': result.returncode,
                                               'wallSeconds': round(time.monotonic() - started, 3)})
    if result.returncode not in allowed:
        raise RuntimeError(f'trusted command failed with exit {result.returncode}; see retained command logs')
    return result


def git(*args, cwd=ROOT, env=None, data=None):
    return execute(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'protocol.file.allow=never', *args], cwd=cwd, env=env, data=data).stdout.decode().strip()


def immutable_image(value):
    if not re.fullmatch(r'(?:[A-Za-z0-9._/:+-]+@)?sha256:[0-9a-f]{64}', value):
        raise ValueError('tooling image must be an immutable digest or captured local image ID')
    return value


def safe_archive(data, destination):
    destination.mkdir(parents=True, exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:') as archive:
        members = archive.getmembers()
        for item in members:
            p = Path(item.name)
            if p.is_absolute() or '..' in p.parts or not (item.isfile() or item.isdir()):
                raise ValueError('source archive contains a nonregular or escaping entry')
        archive.extractall(destination, members=members, filter='data')


def configure(workspace):
    model = os.environ.get('AGENTCTL_MODEL', 'gpt-6-astra')
    if model != 'gpt-6-astra':
        raise ValueError('this reviewed example requires AGENTCTL_MODEL=gpt-6-astra')
    for name in ['remediate', 'eligibility']:
        workflow = load(workspace / 'source/agentctl' / (name + '.yaml'))
        for agent in workflow['spec'].get('agents', {}).values():
            agent['model'] = model
            agent['instructionsFile'] = 'source/agentctl/' + agent['instructionsFile']
        dump(workspace / (name + '.yaml'), workflow)


def container_base(engine, workspace, tooling, online=False, budget=None):
    args = [engine, 'run', '--rm', '--read-only', '--user', f'{os.getuid()}:{os.getgid()}', '--cap-drop=ALL',
            '--security-opt=no-new-privileges', '--pids-limit=128', '--memory=2g', '--cpus=2',
            '--tmpfs', '/tmp:rw,nosuid,size=256m', '--workdir', '/workspace',
            '--mount', f'type=bind,src={workspace},dst=/workspace,readonly',
            '--mount', f'type=bind,src={workspace / "state"},dst=/workspace/state',
            '--mount', f'type=bind,src={workspace / "patch"},dst=/workspace/patch']
    if not online:
        args += ['--network=none']
    if online:
        if not os.environ.get('OPENAI_API_KEY'):
            raise ValueError('OPENAI_API_KEY is required for the explicitly leased live stage')
        if any(not os.environ.get(key) for key in BINDINGS):
            raise ValueError('the live lease requires complete GitHub job identity')
        args += ['--env', 'OPENAI_API_KEY']
        for name in BINDINGS:
            args += ['--env', name]
    if budget and not online:
        for name in BINDINGS:
            args += ['--env', name]
    if budget:
        args += ['--mount', f'type=bind,src={budget},dst=/ci-budget', '--env', 'RUNNER_TEMP=/ci-budget']
    return args + ['--entrypoint', '/usr/bin/python3' if budget else '/usr/local/bin/agentctl', immutable_image(tooling)]


def cli(engine, workspace, tooling, args, label, *, paid=False, budget=None):
    prefix = container_base(engine, workspace, tooling, online=paid, budget=budget)
    command = prefix + ([BUDGET] if budget else []) + args
    result = execute(command, log=workspace.parent/'logs'/label, timeout=660)
    # The budget wrapper forwards the agentctl envelope to stdout, then emits its
    # own usage record; accept the envelope by schema instead of assuming one line.
    envelopes = []
    for line in result.stdout.splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict) and value.get('apiVersion') == 'agentctl.dev/cli/v1':
            envelopes.append(value)
    if not envelopes and not budget:
        value = json.loads(result.stdout)
        if value.get('apiVersion') == 'agentctl.dev/cli/v1':
            envelopes = [value]
    if budget and args[0] == 'receipt': return None
    if len(envelopes) != 1 or not envelopes[0].get('ok'):
        raise ValueError('expected one successful stable agentctl CLI envelope')
    return envelopes[0]



def inspect_replay(engine, workspace, tooling, result, database, label):
    identifier = result['data']['runId']
    original = cli(engine, workspace, tooling, ['--output', 'json', 'inspect', identifier, '--db', database], label+'-inspect')['data']
    if original['run']['state'] != 'succeeded':
        raise ValueError('cannot validate an incomplete agentctl run')
    patch_hashes = {p.name: digest_file(p) for p in (workspace/'patch').iterdir() if p.is_file()}
    replayed = cli(engine, workspace, tooling, ['--output', 'json', 'replay', identifier, '--db', database], label+'-replay')
    replay_id = replayed['data']['runId']
    inspection = cli(engine, workspace, tooling, ['--output', 'json', 'inspect', replay_id, '--db', database], label+'-replay-inspect')['data']
    if inspection['effects'] or inspection['budget']['usage']['providerRequests'] != 0:
        raise ValueError('keyless networkless replay produced fresh effects or provider requests')
    if patch_hashes != {p.name: digest_file(p) for p in (workspace/'patch').iterdir() if p.is_file()}:
        raise ValueError('replay changed staged patch files')
    path = workspace.parent/'runs.json'
    records = json_file(path) if path.exists() else {}
    records['invocation'] = {'repository': os.environ.get('GITHUB_REPOSITORY'), 'sourceSha': os.environ.get('GITHUB_SHA'),
                             'workflowRef': os.environ.get('GITHUB_WORKFLOW_REF'), 'runId': os.environ.get('GITHUB_RUN_ID'),
                             'runAttempt': os.environ.get('GITHUB_RUN_ATTEMPT')}
    records[label] = {'runId': identifier, 'replayRunId': replay_id, 'database': database,
                      'providerRequests': original['budget']['usage']['providerRequests'], 'toolCalls': len(original['toolCalls'])}
    save(path, records)
    return original


def snapshot_db(directory):
    database, metadata = directory/'db/trivy.db', directory/'db/metadata.json'
    if not database.is_file() or not metadata.is_file() or database.is_symlink() or metadata.is_symlink():
        raise ValueError('a fixed Trivy database and metadata snapshot are mandatory')
    return {'databaseSha256': digest_file(database), 'databaseMetadataSha256': digest_file(metadata)}


def scan(engine, image, label, out, policy, snapshot):
    if snapshot_db(out/'trivy-cache') != snapshot:
        raise ValueError('Trivy database snapshot changed before scan')
    archive = out / (label + '-image.tar')
    archive_flags = ['--format', 'docker-archive'] if Path(engine).name == 'podman' else []
    execute([engine, 'save', *archive_flags, '--output', archive, image], log=out/'logs'/(label+'-save'))
    report = out/'workspace/inputs'/(label+'.json')
    command = [engine, 'run', '--rm', '--network=none', '--read-only', '--user', f'{os.getuid()}:{os.getgid()}',
               '--cap-drop=ALL', '--security-opt=no-new-privileges', '--tmpfs', '/tmp:rw,nosuid,size=512m',
               '--mount', f'type=bind,src={out / "trivy-cache"},dst=/cache',
               '--mount', f'type=bind,src={out / "trivy-cache/db"},dst=/cache/db,readonly',
               '--mount', f'type=bind,src={archive},dst=/image.tar,readonly',
               '--mount', f'type=bind,src={report.parent},dst=/reports', policy['scannerImage'],
               'image', '--input', '/image.tar', '--cache-dir', '/cache', '--format', 'json', '--output', '/reports/'+label+'.json',
               '--scanners', 'vuln', '--list-all-pkgs', '--skip-db-update', '--skip-java-db-update', '--offline-scan',
               '--disable-telemetry', '--skip-version-check', '--exit-code', '0', '--timeout', '5m']
    execute(command, log=out/'logs'/(label+'-scan'), timeout=360)
    if snapshot_db(out/'trivy-cache') != snapshot:
        raise ValueError('Trivy database snapshot changed during scan')
    adapter.findings(json_file(report), image)
    return digest_file(report)


def build_test(engine, source, label, out, policy):
    iid = out/(label+'-iid.txt')
    identity = git('rev-parse', 'HEAD', cwd=source) if (source/'.git').exists() else json_file(out/'prepared-source.json')['sourceSha']
    if label == 'after': identity = git('write-tree', cwd=source)
    tag = 'agentctl-remediation-demo:'+label+'-'+identity
    execute([engine, 'build', '--pull=false', '--build-arg', 'PYTHON_BASE='+policy['pythonBaseImage'], '--iidfile', iid, '--tag', tag,
             '--file', source/'Dockerfile', source], log=out/'logs'/(label+'-build'), timeout=600)
    image = adapter.image_id(iid.read_text().strip())
    execute([engine, 'run', '--rm', '--network=none', '--read-only', '--cap-drop=ALL', '--security-opt=no-new-privileges',
             '--tmpfs', '/tmp:rw,nosuid,size=32m', '--entrypoint', 'python', image, '-m', 'unittest', '-v', 'test_app'],
            log=out/'logs'/(label+'-application-tests'), timeout=60)
    return image


def validate_scope():
    policy = adapter.contract()
    source = git('rev-parse', 'HEAD')
    adapter.require_digest(source, 40)
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise ValueError('demo source must be a clean committed checkout')
    if os.environ.get('GITHUB_REPOSITORY', policy['repository']) != policy['repository']:
        raise ValueError('workflow repository is outside the explicitly configured demo')
    if os.environ.get('GITHUB_SHA', source) != source:
        raise ValueError('checked out source does not match this CI run')
    return policy, source, git('rev-parse', 'HEAD^{tree}')


def prepare(args):
    policy, source, tree = validate_scope()
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=False)
    workspace = out/'workspace'
    for name in ['inputs', 'state', 'patch']:
        (workspace/name).mkdir(parents=True)
    archive = execute(['git', 'archive', '--format=tar', source], cwd=ROOT).stdout
    safe_archive(archive, workspace/'source')
    configure(workspace)
    (out/'trivy-cache').mkdir()
    if args.database_snapshot:
        shutil.copytree(args.database_snapshot.resolve(), out/'trivy-cache/db')
    else:
        execute([args.engine, 'run', '--rm', '--user', f'{os.getuid()}:{os.getgid()}', '--cap-drop=ALL',
                 '--mount', f'type=bind,src={out / "trivy-cache"},dst=/cache', policy['scannerImage'],
                 'image', '--download-db-only', '--cache-dir', '/cache', '--disable-telemetry'], log=out/'logs/database-download')
    snapshot = snapshot_db(out/'trivy-cache')
    save(out/'prepared-source.json', {'sourceSha': source})
    before_image = build_test(args.engine, workspace/'source', 'before', out, policy)
    report_sha = scan(args.engine, before_image, 'before', out, policy, snapshot)
    metadata = {'repository': policy['repository'], 'baseBranch': policy['baseBranch'], 'sourceSha': source, 'sourceTree': tree,
                **snapshot, 'imageId': before_image, 'reportSha256': report_sha, 'scannerImage': policy['scannerImage']}
    save(workspace/'inputs/scan-context.json', metadata)
    adapter.ROOT = workspace
    captured = adapter.capture({})
    save(out/'prepared.json', {'sourceSha': source, 'sourceTree': tree, 'decision': captured['decision'], 'toolingImage': immutable_image(args.tooling_image)})
    print(json.dumps({'decision': captured['decision'], 'sourceSha': source}))


def remediate(args):
    out, workspace = args.out.resolve(), args.out.resolve()/'workspace'
    record = json_file(out/'prepared.json')
    if record['decision'] != 'remediate':
        print(json.dumps({'skipped': True, 'reason': record['decision']})); return
    if not args.lease:
        raise ValueError('live remediation requires a preallocated source/run-bound lease')
    shutil.copyfile(args.lease, workspace/'inputs/lease.json')
    budget = out/'ci-budget'; budget.mkdir(exist_ok=True)
    for sub in ['check', 'plan']:
        cli(args.engine, workspace, record['toolingImage'], ['--output', 'json', sub, 'remediate.yaml', '--workspace', '/workspace'], 'remediate-'+sub)
    command = ['execute', '--lease', '/workspace/inputs/lease.json', '--execution-ledger', '/ci-budget/live-budget.sqlite3', '--',
               '/usr/local/bin/agentctl', '--output', 'json', 'run', '/workspace/remediate.yaml', '--workspace', '/workspace', '--db', '/workspace/state/run.sqlite3']
    try:
        outcome = cli(args.engine, workspace, record['toolingImage'], command, 'remediate-run', paid=True, budget=budget)
    finally:
        # A failed receipt remains explicit and never refunds an uncertain lease.
        execute(container_base(args.engine, workspace, record['toolingImage'], budget=budget) + [BUDGET, 'receipt', '--lease', '/workspace/inputs/lease.json',
                '--execution-ledger', '/ci-budget/live-budget.sqlite3'], log=out/'logs/budget-receipt', allowed=(0, 2))
    inspection = inspect_replay(args.engine, workspace, record['toolingImage'], outcome, 'state/run.sqlite3', 'remediation')
    if len(inspection['toolCalls']) != 2:
        raise ValueError('live implementer did not execute exactly the two scoped file tools')
    if not json_file(workspace/'state/patch-ready.json')['ready']:
        raise ValueError('agentctl did not produce a validated patch')


def validate(args):
    out, workspace = args.out.resolve(), args.out.resolve()/'workspace'
    prepared = json_file(out/'prepared.json'); metadata = json_file(workspace/'inputs/scan-context.json')
    policy = adapter.contract()
    gate = {k: metadata[k] for k in ['sourceSha', 'sourceTree', 'databaseSha256', 'databaseMetadataSha256', 'scannerImage', 'reportSha256']}
    if prepared['decision'] == 'remediate':
        patch = json_file(workspace/'state/patch-ready.json')
        candidate = out/'candidate'
        git('worktree', 'add', '--detach', str(candidate), metadata['sourceSha'])
        for name in policy['allowedFiles']:
            data = (workspace/'patch'/name).read_bytes()
            if adapter.sha(data) != patch['files'][name] or data != adapter.expected_files('2.7.0')[name]:
                raise ValueError('outer build refused changed or unauthorized patch bytes')
            (candidate/name).write_bytes(data)
        git('add', '--', *policy['allowedFiles'], cwd=candidate)
        changed = git('diff', '--cached', '--name-only', cwd=candidate).splitlines()
        if sorted(changed) != sorted(policy['allowedFiles']):
            raise ValueError('candidate tree contains unexpected changes')
        tree = git('write-tree', cwd=candidate)
        after_image = build_test(args.engine, candidate, 'after', out, policy)
        snapshot = {k: metadata[k] for k in ['databaseSha256', 'databaseMetadataSha256']}
        report_sha = scan(args.engine, after_image, 'after', out, policy, snapshot)
        gate.update({'testsPassed': True, 'buildSucceeded': True, 'scanSucceeded': True, 'patchDigest': patch['patchDigest'],
                     'validatedTree': tree, 'beforeImageId': metadata['imageId'], 'afterImageId': after_image, 'afterReportSha256': report_sha})
        (out/'validated.patch').write_bytes(execute(['git', 'diff', '--cached', '--binary', '--', *policy['allowedFiles']], cwd=candidate).stdout)
    save(workspace/'inputs/gates.json', gate)
    outcome = cli(args.engine, workspace, prepared['toolingImage'], ['--output', 'json', 'run', 'eligibility.yaml', '--workspace', '/workspace', '--db', 'state/eligibility.sqlite3'], 'eligibility-run')
    inspection = inspect_replay(args.engine, workspace, prepared['toolingImage'], outcome, 'state/eligibility.sqlite3', 'eligibility')
    if inspection['budget']['usage']['providerRequests'] != 0:
        raise ValueError('eligibility unexpectedly requested a provider')
    decision = json_file(workspace/'state/publication.json')
    print(json.dumps({key: decision[key] for key in ['eligible', 'decision', 'sourceSha']} | {'residualCount': len(decision.get('residualFindings', []))}))


def bundle(args):
    out = args.out.resolve()
    destination = out/'publication-bundle'
    destination.mkdir(exist_ok=False)
    for source, target in [('workspace/state/publication.json', 'publication.json'), ('workspace/inputs/gates.json', 'gates.json'),
                           ('workspace/state/context.json', 'context.json'), ('workspace/inputs/before.json', 'before.json'), ('runs.json', 'runs.json')]:
        shutil.copyfile(out/source, destination/target)
    if json_file(destination/'publication.json')['eligible']:
        for source, target in [('workspace/inputs/after.json', 'after.json'), ('workspace/state/patch-ready.json', 'patch-ready.json'), ('validated.patch', 'validated.patch')]:
            shutil.copyfile(out/source, destination/target)
        shutil.copytree(out/'workspace/patch', destination/'patch')
    save(destination/'SHA256SUMS.json', {p.relative_to(destination).as_posix(): digest_file(p) for p in sorted(destination.rglob('*')) if p.is_file()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('stage', choices=['prepare', 'remediate', 'validate', 'bundle'])
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--engine', default='docker')
    parser.add_argument('--tooling-image')
    parser.add_argument('--database-snapshot', type=Path, help='directory containing captured trivy.db and metadata.json')
    parser.add_argument('--lease', type=Path)
    args = parser.parse_args()
    globals()[args.stage](args)

if __name__ == '__main__':
    try: main()
    except Exception as error:
        print(type(error).__name__ + ': ' + str(error), file=sys.stderr)
        sys.exit(1)
