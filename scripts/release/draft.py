#!/usr/bin/env python3
"""Attach complete verified assets to an existing-tag draft; never publish a release."""
import argparse
import json
import subprocess
from pathlib import Path
import tempfile

from artifacts import file_digest
from bundle import identity, verify

REPO = 'opensourceops/agentctl'


def gh(*args):
    return subprocess.check_output(['gh', *args], text=True)


def find_release(tag):
    releases = json.loads(gh('api', '--paginate', '--slurp', 'repos/' + REPO + '/releases?per_page=100'))
    matching = [r for page in releases for r in page if r['tag_name'] == tag]
    if len(matching) > 1:
        raise ValueError('ambiguous release tag')
    return matching[0] if matching else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--directory', required=True, type=Path)
    args = parser.parse_args()
    release = identity('.', args.source, args.tag)
    bundle = verify(args.directory, release)
    resolved = subprocess.check_output(['git', 'rev-parse', 'refs/tags/' + args.tag + '^{commit}'], text=True).strip()
    if resolved != args.source:
        raise ValueError('operator must create the exact matching tag before attaching a draft')
    # Authenticate the durable checksum manifest, not just mutable JSON assertions.
    subprocess.run(['gh', 'attestation', 'verify', str(args.directory / 'release-bundle.json'),
                    '--repo', REPO, '--signer-workflow', REPO + '/.github/workflows/release-prep.yml',
                    '--source-digest', args.source, '--signer-digest', args.source,
                    '--deny-self-hosted-runners', '--bundle', str(args.directory / 'release-bundle.sigstore.json')], check=True)
    record = find_release(args.tag)
    if record is None:
        notes = args.directory / 'release-notes.md'
        notes.write_text(f"Release binaries and OCI archives were prepared from `{args.source}`.\n\n"
                         f"Preparation: https://github.com/{REPO}/actions/runs/{bundle['preparationRunId']}\n\n"
                         "Review every required gate and SHA256SUMS before clicking Publish. Publication promotes the tested Docker Hub images without rebuilding.\n")
        command = ['release', 'create', args.tag, '--repo', REPO, '--verify-tag', '--draft', '--title', 'agentctl ' + args.tag, '--notes-file', str(notes)]
        if release['prerelease']:
            command += ['--prerelease']
        gh(*command)
        notes.unlink()
        # The tag endpoint does not return unpublished drafts. Resolve the exact
        # created draft through the authenticated list, as on a resumed upload.
        record = find_release(args.tag)
        if record is None:
            raise ValueError('created draft could not be located; inspect remote state before retrying')
    if not record['draft']:
        raise ValueError('release is already published; cannot replace immutable release assets')
    if bool(record['prerelease']) != release['prerelease']:
        raise ValueError('draft prerelease setting disagrees with source version')
    existing = {a['name']: a for a in record['assets']}
    for path in sorted(args.directory.iterdir()):
        if not path.is_file() or path.is_symlink():
            raise ValueError('release asset must be a regular file')
        if path.name in existing:
            with tempfile.TemporaryDirectory() as directory:
                gh('release', 'download', args.tag, '--repo', REPO, '--pattern', path.name, '--dir', directory)
                if file_digest(Path(directory) / path.name) != file_digest(path):
                    raise ValueError('existing draft asset differs; preserve it and review the mismatched preparation before replacing anything')
        else:
            # If upload acknowledgement is lost, rerun compares the remote bytes.
            gh('release', 'upload', args.tag, str(path), '--repo', REPO)
    print(record['html_url'])


if __name__ == '__main__':
    main()
