#!/usr/bin/env python3
"""Require successful existing hosted security/CI workflows at the exact source."""
import argparse
import json
import subprocess

from artifacts import SHA

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--source', required=True)
parser.add_argument('--output', required=True)
args = parser.parse_args()
if not SHA.fullmatch(args.source):
    raise ValueError('exact source SHA required')
records = []
for workflow in ['ci.yml', 'container.yml', 'security.yml']:
    endpoint = f'repos/opensourceops/agentctl/actions/workflows/{workflow}/runs?head_sha={args.source}&per_page=100'
    pages = json.loads(subprocess.check_output(['gh', 'api', '--paginate', '--slurp', endpoint]))
    matches = [run for page in pages for run in page['workflow_runs']
               if run['head_sha'] == args.source and run['event'] in {'push', 'pull_request', 'workflow_dispatch'}]
    latest = max(matches, key=lambda run: (run['run_number'], run['run_attempt']), default=None)
    if latest is None or latest['status'] != 'completed' or latest['conclusion'] != 'success':
        raise ValueError(f'{workflow} must finish successfully for {args.source} before assembling release assets')
    records.append({key: latest[key] for key in ['id', 'head_sha', 'html_url', 'conclusion', 'path', 'run_attempt']})
with open(args.output, 'w') as output:
    json.dump({'sourceSha': args.source, 'workflows': records}, output, indent=2)
    output.write('\n')
