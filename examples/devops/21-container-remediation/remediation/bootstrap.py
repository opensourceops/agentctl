#!/usr/bin/env python3
"""Select a published immutable tooling image or build one allowlisted public source."""
import argparse
import json
import os
from pathlib import Path
import re
import tempfile
import tomllib
from runner import execute, git, json_file, immutable_image, save

ROOT = Path(__file__).resolve().parent.parent


def select(mode, requested, engine, output):
    config = json_file(ROOT/'remediation/bootstrap.json')
    framework = None
    if mode == 'published':
        image = immutable_image(requested)
        if not image.startswith(config['toolingRegistry']+'@sha256:'):
            raise ValueError('published image must use the configured Docker Hub repository and exact digest')
        execute([engine, 'pull', image], log=output.with_name('tooling-pull'))
    elif mode == 'candidate':
        if requested not in config['allowedFrameworkShas'] or not re.fullmatch('[0-9a-f]{40}', requested):
            raise ValueError('candidate source is not the explicitly reviewed framework commit')
        framework = requested
        with tempfile.TemporaryDirectory(prefix='agentctl-tooling-source-') as temporary:
            source = Path(temporary)
            git('init', cwd=source)
            git('remote', 'add', 'origin', 'https://github.com/'+config['frameworkRepository']+'.git', cwd=source)
            git('fetch', '--depth=1', 'origin', requested, cwd=source)
            git('checkout', '--detach', 'FETCH_HEAD', cwd=source)
            if git('rev-parse', 'HEAD', cwd=source) != requested:
                raise ValueError('public checkout identity differs from the requested candidate')
            version = tomllib.loads((source/'Cargo.toml').read_text())['workspace']['package']['version']
            iid = source/'tooling.iid'
            execute([engine, 'build', '--target', 'tooling', '--file', source/'Containerfile', '--iidfile', iid,
                     '--build-arg', 'AGENTCTL_REVISION='+requested, '--build-arg', 'AGENTCTL_VERSION='+version, source],
                    log=output.with_name('tooling-build'), timeout=1800)
            image = immutable_image(iid.read_text().strip())
    else:
        raise ValueError('unknown tooling selection mode')
    inspection = json.loads(execute([engine, 'image', 'inspect', image]).stdout)[0]
    labels = inspection['Config'].get('Labels') or {}
    if labels.get('dev.agentctl.variant') != 'tooling' or labels.get('org.opencontainers.image.source') != 'https://github.com/opensourceops/agentctl':
        raise ValueError('selected image does not identify the required agentctl tooling flavor')
    if framework and labels.get('org.opencontainers.image.revision') != framework:
        raise ValueError('tooling image revision does not match the captured framework source')
    save(output, {'mode': mode, 'frameworkSha': framework or labels.get('org.opencontainers.image.revision'),
                  'image': image, 'imageId': inspection['Id'], 'repoDigests': inspection.get('RepoDigests', []), 'labels': labels})
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as handle:
            handle.write('image='+image+'\n')
    print(json.dumps({'image': image, 'frameworkSha': framework}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mode', choices=['candidate', 'published'], required=True)
    parser.add_argument('--reference', required=True)
    parser.add_argument('--engine', default='docker')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args(); args.output.parent.mkdir(parents=True, exist_ok=True)
    select(args.mode, args.reference, args.engine, args.output)

if __name__ == '__main__': main()
