#!/usr/bin/env python3
"""Export the complete demo; destination must not exist. No Git or network writes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

SOURCE = Path(__file__).resolve().parent.parent
IGNORED = {'__pycache__', '.git', '.venv', 'state'}


def export(destination, framework_sha, allow_dirty_preview=False, archive=None):
    if not re.fullmatch(r'[0-9a-f]{40}', framework_sha):
        raise ValueError('framework SHA must be an exact reviewed lowercase commit')
    status = subprocess.run(['git', 'status', '--porcelain', '--', str(SOURCE)], cwd=SOURCE, capture_output=True, text=True, check=True).stdout
    head = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=SOURCE, capture_output=True, text=True, check=True).stdout.strip()
    dirty = bool(status) or head != framework_sha
    if dirty and not allow_dirty_preview:
        raise ValueError('export requires clean package bytes at the exact framework HEAD; use --allow-dirty-preview only for labeled local previews')
    if archive and Path(archive).exists():
        raise ValueError('archive already exists; refusing to overwrite it')
    destination = Path(destination).resolve()
    if destination.exists():
        raise ValueError('export destination already exists; refusing to overwrite it')
    destination.mkdir(parents=True)
    for source in sorted(SOURCE.rglob('*')):
        relative = source.relative_to(SOURCE)
        if any(part in IGNORED for part in relative.parts) or source.suffix == '.pyc': continue
        if source.is_symlink(): raise ValueError('package cannot contain symlinks')
        if source.is_file():
            target = destination/relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
    config = {'schemaVersion': 1, 'frameworkRepository': 'opensourceops/agentctl', 'allowedFrameworkShas': [framework_sha],
              'toolingRegistry': 'docker.io/opensourceops/agentctl', 'repository': 'Ompragash/agentctl-remediation-demo'}
    (destination/'remediation/bootstrap.json').write_text(json.dumps(config, indent=2)+'\n')
    files = {p.relative_to(destination).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in sorted(destination.rglob('*')) if p.is_file()}
    manifest = {'schemaVersion': 1, 'frameworkSourceSha': framework_sha, 'sourceDirty': dirty, 'frameworkPackagePath': 'examples/devops/21-container-remediation', 'files': files}
    (destination/'package-manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True)+'\n')
    if archive:
        archive = Path(archive)
        archive.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(archive, 'x', compression=zipfile.ZIP_DEFLATED, compresslevel=9) as output:
            for source in sorted(destination.rglob('*')):
                if not source.is_file(): continue
                name = '21-container-remediation/' + source.relative_to(destination).as_posix()
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = (0o100000 | (source.stat().st_mode & 0o777)) << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                output.writestr(info, source.read_bytes(), compresslevel=9)
    return manifest


def verify(root, allow_dirty_preview=False):
    root = Path(root).resolve()
    manifest = json.loads((root/'package-manifest.json').read_text())
    if manifest.get('sourceDirty') is not False and not allow_dirty_preview:
        raise ValueError('dirty preview export is not committed source evidence')
    actual = {p.relative_to(root).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*')
              if p.is_file() and p.name != 'package-manifest.json' and not any(part in IGNORED for part in p.relative_to(root).parts) and p.suffix != '.pyc'}
    if actual != manifest['files']: raise ValueError('export differs from its recorded package contents')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--framework-sha')
    parser.add_argument('--verify', type=Path)
    parser.add_argument('--archive', type=Path, help='optional deterministic ZIP, with a 21-container-remediation root directory')
    parser.add_argument('--allow-dirty-preview', action='store_true', help='explicitly label local previews; unsuitable for live CI')
    args = parser.parse_args()
    if args.verify: result = verify(args.verify, args.allow_dirty_preview)
    elif args.output and args.framework_sha: result = export(args.output, args.framework_sha, args.allow_dirty_preview, args.archive)
    else: parser.error('use --output DIRECTORY --framework-sha SHA or --verify DIRECTORY')
    print(json.dumps({'frameworkSourceSha': result['frameworkSourceSha'], 'files': len(result['files'])}))

if __name__ == '__main__': main()
