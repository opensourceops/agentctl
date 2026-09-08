#!/usr/bin/env python3
"""Separate source/job-bound live lease: actual Responses tool and strict output probe."""
import argparse
import json
from pathlib import Path
import os
import shutil
import runner
from adapter import expected_files
from yaml_io import load, dump


def expected_echo_input():
    files = expected_files('2.7.0')
    return {name: {'path': 'patch/' + filename, 'content': files[filename].decode()}
            for name, filename in [('manifest', 'requirements.in'), ('lock', 'requirements.lock')]}


def configure(workspace):
    workflow = load(workspace/'source/agentctl/preflight.yaml')
    model = os.environ.get('AGENTCTL_MODEL', 'gpt-6-astra')
    if model != 'gpt-6-astra':
        raise ValueError('preflight requires AGENTCTL_MODEL=gpt-6-astra')
    workflow['spec']['agents']['probe']['model'] = model
    workflow['spec']['agents']['probe']['instructionsFile'] = 'source/agentctl/instructions/preflight.md'
    dump(workspace/'preflight.yaml', workflow)
    return workflow


def assert_compatibility(inspection):
    if inspection['run']['state'] != 'succeeded' or inspection['run']['output'] != {'preflight': {'echo': 'ok'}}:
        raise ValueError('strict structured preflight output does not match the expected echoed value')
    calls = inspection.get('toolCalls', [])
    if len(calls) != 1 or calls[0].get('toolId') != 'echo' or calls[0].get('status') != 'succeeded':
        raise ValueError('preflight did not complete exactly one real echo tool call')
    effects = {effect['request']['id']: effect for effect in inspection.get('effects', [])}
    effect = effects.get(calls[0]['effectId'], {})
    expected = expected_echo_input()
    if effect.get('status') != 'succeeded' or effect.get('request', {}).get('input') != expected or effect.get('result') != expected:
        raise ValueError('durable tool input/result did not preserve the exact scoped value')
    usage = inspection['budget']['usage']
    if (not 2 <= usage['providerRequests'] <= 3 or usage['inputTokens'] + usage['outputTokens'] > 16000
            or usage['costMicrousd'] > 600000 or usage['wallTimeSeconds'] > 90):
        raise ValueError('preflight usage exceeds its reviewed request/token/cost/time ceiling')
    return usage


def run(args):
    policy, source, _ = runner.validate_scope()
    out = args.out.resolve(); out.mkdir(parents=True, exist_ok=False)
    workspace = out/'workspace'
    for directory in ['inputs', 'state', 'patch']:
        (workspace/directory).mkdir(parents=True)
    archive = runner.execute(['git', 'archive', '--format=tar', source], cwd=runner.ROOT).stdout
    runner.safe_archive(archive, workspace/'source')
    configure(workspace)
    shutil.copyfile(args.lease, workspace/'inputs/lease.json')
    budget = out/'ci-budget'; budget.mkdir()
    tooling = runner.immutable_image(args.tooling_image)
    runner.save(out/'identity.json', {'repository': policy['repository'], 'sourceSha': source, 'toolingImage': tooling,
                                    'operation': 'Responses tool/structured-output preflight', 'live': True})
    for command in ['check', 'plan']:
        runner.cli(args.engine, workspace, tooling, ['--output', 'json', command, 'preflight.yaml', '--workspace', '/workspace'], command)
    arguments = ['execute', '--lease', '/workspace/inputs/lease.json', '--execution-ledger', '/ci-budget/live-budget.sqlite3', '--',
                 '/usr/local/bin/agentctl', '--output', 'json', 'run', '/workspace/preflight.yaml', '--workspace', '/workspace', '--db', '/workspace/state/preflight.sqlite3']
    try:
        outcome = runner.cli(args.engine, workspace, tooling, arguments, 'preflight-run', paid=True, budget=budget)
    finally:
        runner.execute(runner.container_base(args.engine, workspace, tooling, budget=budget) + [runner.BUDGET, 'receipt',
                       '--lease', '/workspace/inputs/lease.json', '--execution-ledger', '/ci-budget/live-budget.sqlite3'],
                       log=out/'logs/budget-receipt', allowed=(0, 2))
    inspection = runner.inspect_replay(args.engine, workspace, tooling, outcome, 'state/preflight.sqlite3', 'preflight')
    usage = assert_compatibility(inspection)
    result = {'passed': True, 'sourceSha': source, 'toolingImage': tooling, 'model': 'gpt-6-astra', 'reasoning': 'high',
              'runId': outcome['data']['runId'], 'usage': usage, 'structuredOutput': {'echo': 'ok'}, 'toolCalls': 1, 'replayFreshEffects': 0}
    runner.save(out/'result.json', result)
    print(json.dumps(result))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True); parser.add_argument('--lease', type=Path, required=True)
    parser.add_argument('--tooling-image', required=True); parser.add_argument('--engine', default='docker')
    run(parser.parse_args())

if __name__ == '__main__': main()
