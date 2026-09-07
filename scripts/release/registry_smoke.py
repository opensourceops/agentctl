#!/usr/bin/env python3
"""Pull both platform manifests from the assembled disposable registry index."""
import argparse
import json
import subprocess
from pathlib import Path

from publish import Registry
from bundle import write_json

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--bundle', required=True)
parser.add_argument('--registry', required=True)
parser.add_argument('--output', required=True)
args = parser.parse_args()
registry = Registry(args.registry, test=True)
bundle = json.loads(Path(args.bundle).read_text())
results = []
for variant in bundle['images']:
    reference = args.registry + '@' + variant['manifestDigest']
    for image in variant['platforms']:
        # Native execution already occurred in each architecture job. This checks
        # that the newly assembled exact index delivers each tested platform.
        subprocess.run(['docker', 'pull', '--platform', image['platform'], reference], check=True, timeout=180)
        try:
            actual = json.loads(subprocess.check_output(['docker', 'image', 'inspect', reference]))[0]
            if actual['Id'] != image['configDigest'] or actual['Os'] + '/' + actual['Architecture'] != image['platform']:
                raise ValueError('registry index did not deliver the tested native platform/config')
            results.append({'variant': variant['variant'], 'platform': image['platform'], 'manifestDigest': variant['manifestDigest'], 'imageId': actual['Id']})
        finally:
            # Docker's classic store maps one index reference to one local image.
            # Release only this probe's reference before pulling another platform.
            subprocess.run(['docker', 'image', 'rm', reference], check=True, timeout=60)
write_json(args.output, {'passed': True, 'nativeExecution': 'separate native image jobs', 'pulls': results})
