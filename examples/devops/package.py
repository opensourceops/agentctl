#!/usr/bin/env python3
"""Copy a complete cookbook example; optionally create a deterministic ZIP (stdlib only)."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
from urllib.parse import quote, unquote, urlsplit
import zipfile

ROOT = Path(__file__).resolve().parent
SUPPORT = {"fixture.py": "helper.py", "yaml_io.py": "yaml_io.py", "requirements.txt": "requirements.txt",
           "setup_example.py": "setup.py", "operations.py": "operations.py", "format_operations.py": "format_operations.py",
           "local_service.py": "local_service.py", "service_operations.py": "service_operations.py"}


def render_readme_links(text, source, destination, revision):
    """Pin informational links to source when their targets are outside the package."""
    framework = ROOT.parent.parent.resolve()
    destination = destination.resolve()
    link = re.compile(r'(?<!!)\[[^\]\n]*\]\((?P<target><[^>\n]+>|[^\s()]+)(?:\s+"[^"\n]*")?\)')
    fence = None
    rendered = []
    for line in text.splitlines(keepends=True):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})", line)
        if marker:
            if fence is None:
                fence = marker[1]
            elif marker[1][0] == fence[0] and len(marker[1]) >= len(fence):
                fence = None
            rendered.append(line)
            continue
        if fence is not None:
            rendered.append(line)
            continue
        code = [match.span() for match in re.finditer(r"(`+).*?\1", line)]

        def replace(match):
            if any(start <= match.start() < end for start, end in code):
                return match[0]
            raw = match["target"]
            target = urlsplit(raw[1:-1] if raw.startswith("<") else raw)
            if target.scheme or target.netloc or not target.path or target.path.startswith("/"):
                return match[0]
            relative = unquote(target.path)
            copied = (destination / relative).resolve()
            if copied.is_relative_to(destination) and copied.exists():
                return match[0]
            original = (source / relative).resolve()
            if not original.is_relative_to(framework) or not original.is_file():
                return match[0]
            url = "https://github.com/opensourceops/agentctl/blob/" + revision + "/" + quote(original.relative_to(framework).as_posix(), safe="/")
            if target.query:
                url += "?" + target.query
            if target.fragment:
                url += "#" + target.fragment
            start, end = match.span("target")
            return match[0][:start - match.start()] + url + match[0][end - match.start():]

        rendered.append(link.sub(replace, line))
    return "".join(rendered)


def package_example(identifier, output, archive=None):
    entries = json.loads((ROOT / "catalog.json").read_text(encoding="utf-8"))["examples"]
    selected = next((item for item in entries if item["id"] == identifier), None)
    if selected is None:
        raise ValueError(f"unknown example {identifier}; expected 01 through 20")
    destination = Path(output).absolute()
    if destination.exists():
        raise ValueError("package destination must be a new directory; existing work is never replaced")
    source = ROOT / selected["directory"]
    if destination.is_relative_to(source):
        raise ValueError("package destination must be outside its authored example")
    shutil.copytree(source, destination, ignore=shutil.ignore_patterns("__pycache__", "*.pyc", "*.db", "*.db-*", ".venv", "artifacts", "evidence", "local.*.yaml"))
    for filename, target in SUPPORT.items():
        shutil.copy2(ROOT / filename, destination / target)
    revision = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
    readme = destination / "README.md"
    if readme.is_file():
        rendered = render_readme_links(readme.read_bytes().decode("utf-8"), source, destination, revision)
        readme.write_bytes(rendered.encode("utf-8"))
    files = {str(p.relative_to(destination)): hashlib.sha256(p.read_bytes()).hexdigest()
             for p in sorted(destination.rglob("*")) if p.is_file()}
    metadata = {"schemaVersion": "agentctl.dev/cookbook-package/v1", "exampleId": identifier,
                "directory": selected["directory"], "sourceSha": revision, "sourceWorktreeDirty": bool(dirty),
                "dependencies": selected["dependencies"], "files": files,
                "setup": "Create a Python virtual environment, install requirements.txt, then run setup.py with that interpreter."}
    (destination / "example.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    if archive:
        archive = Path(archive)
        if archive.exists():
            raise ValueError("archive destination already exists")
        archive.parent.mkdir(parents=True, exist_ok=True)
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zipped:
            for filename in sorted(destination.rglob("*")):
                if filename.is_file():
                    info = zipfile.ZipInfo(str(Path(selected["directory"]) / filename.relative_to(destination)), (1980, 1, 1, 0, 0, 0))
                    info.create_system = 3
                    info.external_attr = 0o100644 << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    zipped.writestr(info, filename.read_bytes())
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--example", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--archive", type=Path)
    args = parser.parse_args()
    metadata = package_example(args.example, args.output, args.archive)
    print(json.dumps({"example": metadata["exampleId"], "directory": str(args.output.absolute()),
                      "sourceSha": metadata["sourceSha"], "archive": str(args.archive) if args.archive else None}))


if __name__ == "__main__":
    main()
