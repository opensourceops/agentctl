#!/usr/bin/env python3
"""Read-only authorization of a prior exact-run publication artifact; no model calls."""
import argparse
import json
import os
from pathlib import Path
import re
import adapter
from publisher import GitHub
from runner import save


def verify_run(api, run_id, attempt, source_sha):
    if any(not re.fullmatch(r'[1-9][0-9]*', str(value)) for value in [run_id, attempt]):
        raise ValueError('positive prior CI run and attempt IDs are required')
    adapter.require_digest(source_sha, 40)
    run = api.request('GET', f'actions/runs/{run_id}/attempts/{attempt}')
    policy = adapter.contract()
    if not isinstance(run, dict) or run.get('repository', {}).get('full_name') != policy['repository']:
        raise ValueError('prior execution belongs to another repository')
    if run.get('head_sha') != source_sha or run.get('path') != '.github/workflows/remediation.yml' or run.get('head_branch') != policy['baseBranch']:
        raise ValueError('prior execution source, workflow or branch differs')
    if run.get('event') != 'workflow_dispatch' or run.get('status') != 'completed' or run.get('run_attempt') != int(attempt):
        raise ValueError('prior execution is not this completed dispatched attempt')
    jobs = api.request('GET', f'actions/runs/{run_id}/attempts/{attempt}/jobs?per_page=100')
    if not isinstance(jobs, dict) or jobs.get('total_count', 101) > 100:
        raise ValueError('prior job inventory is missing or truncated')
    matching = [job for job in jobs.get('jobs', []) if job.get('name') == 'remediate']
    if len(matching) != 1 or matching[0].get('conclusion') != 'success':
        raise ValueError('the exact prior validation/remediation job did not succeed')
    artifacts = api.request('GET', f'actions/runs/{run_id}/artifacts?per_page=100')
    if not isinstance(artifacts, dict) or artifacts.get('total_count', 101) > 100:
        raise ValueError('prior artifact inventory is missing or truncated')
    name = f'publication-{run_id}-{attempt}'
    matching = [artifact for artifact in artifacts.get('artifacts', []) if artifact.get('name') == name]
    if len(matching) != 1 or matching[0].get('expired') is not False or matching[0].get('workflow_run', {}).get('head_sha') != source_sha:
        raise ValueError('exact unexpired publication artifact source is not established')
    digest = matching[0].get('digest', '')
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', digest):
        raise ValueError('publication artifact lacks a recorded digest')
    return {'sourceSha': source_sha, 'runId': str(run_id), 'runAttempt': str(attempt), 'artifactId': matching[0]['id'], 'artifactName': name,
            'artifactDigest': digest, 'remediationJobConclusion': 'success', 'priorWorkflowConclusion': run.get('conclusion')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run-id', required=True); parser.add_argument('--attempt', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    policy = adapter.contract()
    if os.environ.get('GITHUB_REPOSITORY') != policy['repository']:
        raise ValueError('reconciliation job is outside the configured demo')
    api = GitHub(policy['repository'], os.environ.get('GITHUB_TOKEN'))
    result = verify_run(api, args.run_id, args.attempt, os.environ['GITHUB_SHA'])
    save(args.output, result)
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as handle:
            handle.write('artifact-id='+str(result['artifactId'])+'\n')
    print(json.dumps(result))

if __name__ == '__main__': main()
