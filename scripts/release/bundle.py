#!/usr/bin/env python3
"""Build and verify complete release assets before a GitHub release is published."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

from artifacts import (OciArchive, PLATFORMS, TARGETS, VARIANTS, archive_package,
                       file_digest, merge_oci_archives, release_identity)


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def load(path):
    return json.loads(Path(path).read_text())


def identity(root, source, tag):
    result = release_identity(root, source, tag)
    actual = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if actual != source:
        raise ValueError("checkout does not match the exact requested release source")
    return result


def sbom(path, binary=False):
    value = load(path)
    components = value.get("components")
    if value.get("bomFormat") != "CycloneDX" or not isinstance(components, list) or not components or any(not isinstance(component, dict) for component in components):
        raise ValueError("missing or empty CycloneDX SBOM")
    if binary and value.get("metadata", {}).get("component", {}).get("name") != "agentctl":
        raise ValueError("production SBOM does not identify agentctl")


def scan(path, config_digest):
    value = load(path)
    if value.get("SchemaVersion") != 2 or value.get("Metadata", {}).get("ImageID") != config_digest:
        raise ValueError("scan report does not bind the tested image config")
    if value.get("ArtifactType") != "container_image" or not isinstance(value.get("Results"), list):
        raise ValueError("invalid image vulnerability report")
    blocked = [item for result in value["Results"] for item in result.get("Vulnerabilities", []) or []
               if item.get("Severity") in {"HIGH", "CRITICAL"} and item.get("FixedVersion")]
    if blocked:
        raise ValueError("fixed HIGH/CRITICAL image vulnerabilities block release")


def example_contract(path, config_digest):
    value = load(path)
    recovery = value.get("recovery", {})
    failures = recovery.get("failedValidations", [])
    if (value.get("status") != "passed" or value.get("toolingImage") != config_digest
            or value.get("toolCalls") != 2 or value.get("providerNetworkRequests") != 0
            or value.get("replayFreshEffects") != 0
            or value.get("preflight", {}).get("replayFreshEffects") != 0
            or recovery.get("reusedTasks") != ["capture", "analyze"]
            or recovery.get("freshAnalyzerEffects") != 0 or recovery.get("replayFreshEffects") != 0
            or len(failures) != 3
            or {item.get("gate") for item in failures} != {"testsPassed", "buildSucceeded", "scanSucceeded"}
            or any(item.get("exitCode") != 4 or item.get("publicationFileExists") is not False for item in failures)):
        raise ValueError("container remediation contract evidence failed or does not match the tested image")


def image_evidence_fields(record):
    return ["scan", "sbom", "smoke"] + (["exampleContract"] if record["variant"] == "tooling" else [])


def package(args, release):
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(args.root) / "dist" / f"agentctl-{release['version']}-{args.target}"
    binary = directory / ("agentctl.exe" if args.target.endswith("windows-msvc") else "agentctl")
    version = subprocess.check_output([str(binary.resolve()), "--version"], text=True).strip()
    if version != "agentctl " + release["version"]:
        raise ValueError("packaged executable version does not match source")
    # Exercise the packaged executable, rather than a target/debug substitute.
    import tempfile
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        shutil.copy(Path(args.root) / "examples/v1/hello.yaml", root / "hello.yaml")
        completed = subprocess.run([str(binary.resolve()), "run", str(root / "hello.yaml"),
                                   "--workspace", str(root), "--db", str(root / "run.db"),
                                   "--output", "json"], check=True, capture_output=True, text=True)
        envelope = json.loads(completed.stdout)
        if envelope.get("ok") is not True or envelope.get("data", {}).get("state") != "succeeded" or envelope["data"].get("output", {}).get("greeting") != "hello, world":
            raise ValueError("packaged hello workflow did not succeed")
    extension = ".zip" if args.target.endswith("windows-msvc") else ".tar.gz"
    record = archive_package(directory, args.target, release["version"], output / (directory.name + extension))
    sbom(args.sbom, binary=True)
    sbom_name = directory.name + ".cdx.json"
    shutil.copyfile(args.sbom, output / sbom_name)
    record.update(identity=release, sbom=sbom_name, sbomSha256=file_digest(output / sbom_name), gates=["verify", "acceptance", "completeness", "package", "packaged-hello"])
    write_json(output / (directory.name + ".record.json"), record)


def image(args, release):
    with OciArchive(args.archive) as archive:
        verified = archive.images(release, args.variant)
    if len(verified["images"]) != 1 or verified["images"][0]["platform"] != args.platform:
        raise ValueError("native image job must contain exactly its declared runnable platform")
    if not verified["attestations"]:
        raise ValueError("BuildKit provenance is required")
    tested = verified["images"][0]
    scan(args.scan, tested["configDigest"])
    sbom(args.sbom)
    smoke = load(args.smoke)
    if smoke.get("imageId") != tested["configDigest"] or smoke.get("platform") != args.platform or smoke.get("passed") is not True:
        raise ValueError("native runtime/replay evidence does not match the OCI image")
    record = {"identity": release, "variant": args.variant, "platform": args.platform,
              "archive": Path(args.archive).name, "archiveSha256": file_digest(args.archive),
              "image": tested, "scan": Path(args.scan).name, "sbom": Path(args.sbom).name,
              "smoke": Path(args.smoke).name, "provenance": verified["attestations"],
              "scanSha256": file_digest(args.scan), "sbomSha256": file_digest(args.sbom), "smokeSha256": file_digest(args.smoke)}
    if args.variant == "tooling":
        if not args.example_contract:
            raise ValueError("tooling images require the container remediation contract gate")
        example_contract(args.example_contract, tested["configDigest"])
        record.update(exampleContract=Path(args.example_contract).name,
                      exampleContractSha256=file_digest(args.example_contract))
    write_json(args.output, record)


def assemble(args, release):
    incoming, output = Path(args.incoming), Path(args.output)
    output.mkdir(parents=True, exist_ok=False)
    records = [load(path) for path in incoming.rglob("*.record.json")]
    if any(record["identity"] != release for record in records):
        raise ValueError("preparation artifacts mix source identities")
    packages = [r for r in records if "target" in r]
    images = [r for r in records if "variant" in r]
    if len(packages) != 4 or {r["target"] for r in packages} != TARGETS:
        raise ValueError("all four native CLI packages must pass before release preparation")
    if len(images) != 4 or {(r["variant"], r["platform"]) for r in images} != {(v,p) for v in VARIANTS for p in PLATFORMS}:
        raise ValueError("both image variants and both native architectures must pass")
    files = {}
    for path in incoming.rglob("*"):
        if path.is_file():
            if path.name in files or path.is_symlink():
                raise ValueError("ambiguous incoming artifact names")
            files[path.name] = path
    for record in packages:
        if file_digest(files[record["file"]]) != record["sha256"]:
            raise ValueError("package transport checksum mismatch")
        sbom(files[record["sbom"]], binary=True)
        if file_digest(files[record["sbom"]]) != record["sbomSha256"]:
            raise ValueError("package SBOM transport checksum mismatch")
    for record in images:
        if file_digest(files[record["archive"]]) != record["archiveSha256"]:
            raise ValueError("OCI transport checksum mismatch")
        scan(files[record["scan"]], record["image"]["configDigest"])
        sbom(files[record["sbom"]])
        for field in image_evidence_fields(record):
            if file_digest(files[record[field]]) != record[field + "Sha256"]:
                raise ValueError("image evidence transport checksum mismatch")
        smoke = load(files[record["smoke"]])
        if smoke.get("passed") is not True or smoke.get("platform") != record["platform"] or smoke.get("imageId") != record["image"]["configDigest"]:
            raise ValueError("native runtime evidence failed or changed")
        if record["variant"] == "tooling":
            example_contract(files[record["exampleContract"]], record["image"]["configDigest"])
    # Individual archives are replaced by one complete OCI archive per variant.
    for name, path in files.items():
        if not name.endswith(".oci.tar"):
            shutil.copyfile(path, output / name)
    manifests = []
    for variant in sorted(VARIANTS):
        paths = [files[r["archive"]] for r in images if r["variant"] == variant]
        manifests.append(merge_oci_archives(paths, output / f"agentctl-{release['version']}-{variant}.oci.tar", release, variant))
    assets = [{"file": p.name, "sha256": file_digest(p), "bytes": p.stat().st_size} for p in sorted(output.iterdir())]
    document = {"schemaVersion": 1, "identity": release, "repository": "opensourceops/agentctl",
                "preparationRunId": os.environ["GITHUB_RUN_ID"], "preparationAttempt": os.environ["GITHUB_RUN_ATTEMPT"],
                "registry": "docker.io/opensourceops/agentctl", "packages": packages, "images": manifests, "nativeImages": images, "assets": assets}
    write_json(output / "release-bundle.json", document)
    (output / "SHA256SUMS").write_text("".join(f"{file_digest(p)}  {p.name}\n" for p in sorted(output.iterdir()) if p.is_file()))
    verify(output, release)


def verify(directory, release):
    directory = Path(directory)
    bundle = load(directory / "release-bundle.json")
    if bundle.get("schemaVersion") != 1 or bundle.get("identity") != release or bundle.get("registry") != "docker.io/opensourceops/agentctl":
        raise ValueError("prepared bundle does not match release tag/source/registry; run release preparation first")
    if {p["target"] for p in bundle["packages"]} != TARGETS or len(bundle["packages"]) != 4:
        raise ValueError("prepared release is missing native packages")
    if {p["variant"] for p in bundle["images"]} != VARIANTS or len(bundle["images"]) != 2:
        raise ValueError("prepared release is missing image variants")
    names = set()
    for asset in bundle["assets"]:
        name = asset["file"]
        if Path(name).name != name or name in names or name.startswith("."):
            raise ValueError("unsafe or duplicate release asset")
        names.add(name)
        path = directory / name
        if not path.is_file() or path.is_symlink() or file_digest(path) != asset["sha256"] or path.stat().st_size != asset["bytes"]:
            raise ValueError("release asset missing or checksum mismatch")
    for record in bundle["images"]:
        if record["file"] not in names:
            raise ValueError("OCI archive is not covered by the asset checksum manifest")
        with OciArchive(directory / record["file"]) as archive:
            actual = archive.images(release, record["variant"])
            descriptor = archive.index["manifests"]
            if len(descriptor) != 1 or descriptor[0]["digest"] != record["manifestDigest"]:
                raise ValueError("multi-platform index identity mismatch")
            if {i["platform"] for i in actual["images"]} != PLATFORMS or not actual["attestations"]:
                raise ValueError("incomplete runnable platforms/provenance")
            actual_platforms = {i["platform"]: (i["descriptor"]["digest"], i["configDigest"]) for i in actual["images"]}
            recorded_platforms = {i["platform"]: (i["manifestDigest"], i["configDigest"]) for i in record["platforms"]}
            if len(record["platforms"]) != 2 or actual_platforms != recorded_platforms:
                raise ValueError("platform evidence does not identify the actual OCI images")
    for record in bundle["packages"]:
        if record.get("identity") != release or not {record["file"], record["sbom"]} <= names:
            raise ValueError("package archive/SBOM is absent from the asset manifest")
        if file_digest(directory / record["file"]) != record["sha256"] or file_digest(directory / record["sbom"]) != record["sbomSha256"]:
            raise ValueError("package evidence checksum mismatch")
        sbom(directory / record["sbom"], binary=True)
        if record.get("gates") != ["verify", "acceptance", "completeness", "package", "packaged-hello"]:
            raise ValueError("package gate inventory is incomplete")
    native = bundle.get("nativeImages", [])
    if len(native) != 4 or {(r["variant"], r["platform"]) for r in native} != {(v,p) for v in VARIANTS for p in PLATFORMS}:
        raise ValueError("native image evidence inventory is incomplete")
    for record in native:
        if record.get("identity") != release:
            raise ValueError("native image evidence uses different source identity")
        for field in image_evidence_fields(record):
            if record[field] not in names or file_digest(directory / record[field]) != record[field + "Sha256"]:
                raise ValueError("native image evidence checksum mismatch")
        scan(directory / record["scan"], record["image"]["configDigest"])
        sbom(directory / record["sbom"])
        smoke = load(directory / record["smoke"])
        if smoke.get("passed") is not True or smoke.get("platform") != record["platform"] or smoke.get("imageId") != record["image"]["configDigest"]:
            raise ValueError("native runtime evidence failed or changed")
        if record["variant"] == "tooling":
            example_contract(directory / record["exampleContract"], record["image"]["configDigest"])
        combined = next(r for r in bundle["images"] if r["variant"] == record["variant"])
        expected = next(p for p in combined["platforms"] if p["platform"] == record["platform"])
        if expected["configDigest"] != record["image"]["configDigest"] or expected["manifestDigest"] != record["image"]["descriptor"]["digest"]:
            raise ValueError("tested native image does not match the assembled manifest")
    return bundle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=".")
    parser.add_argument("--source", required=True)
    parser.add_argument("--tag", required=True)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("identity")
    command = commands.add_parser("package")
    command.add_argument("--target", required=True, choices=sorted(TARGETS))
    command.add_argument("--sbom", required=True)
    command.add_argument("--output", required=True)
    command = commands.add_parser("image")
    for field in ["archive", "variant", "platform", "scan", "sbom", "smoke", "output"]:
        command.add_argument("--" + field, required=True)
    command.add_argument("--example-contract")
    command = commands.add_parser("assemble")
    command.add_argument("--incoming", required=True)
    command.add_argument("--output", required=True)
    command = commands.add_parser("verify")
    command.add_argument("--directory", required=True)
    args = parser.parse_args()
    release = identity(args.root, args.source, args.tag)
    if args.command == "identity":
        print(json.dumps(release))
    elif args.command == "verify":
        print(json.dumps(verify(args.directory, release)))
    else:
        globals()[args.command](args, release)


if __name__ == "__main__":
    main()
