#!/usr/bin/env python3
"""Promote verified OCI archives, reconciling immutable version tags before aliases."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess

from artifacts import file_digest, image_tags, sha256, version_parts
from bundle import identity, verify, write_json

PRODUCTION = "docker.io/opensourceops/agentctl"


class Registry:
    def __init__(self, repository, test=False):
        if test:
            if not re.fullmatch(r"(?:localhost|127\.0\.0\.1):[0-9]+/agentctl(?:-[a-z0-9-]+)?", repository):
                raise ValueError("disposable registry must use the loopback interface and an agentctl repository")
        elif repository != PRODUCTION:
            raise ValueError("production publication is restricted to Docker Hub opensourceops/agentctl")
        self.repository, self.test = repository, test

    def reference(self, tag):
        if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}", tag):
            raise ValueError("invalid registry tag")
        return f"docker://{self.repository}:{tag}"

    def inspect(self, tag, config=False):
        command = ["skopeo", "inspect"] + (["--tls-verify=false"] if self.test else [])
        command += ["--config" if config else "--raw", self.reference(tag)]
        result = subprocess.run(command, capture_output=True, timeout=120)
        if result.returncode:
            # Authentication, TLS, rate limits and timeouts are not an absent tag.
            if re.search(rb"manifest unknown|name unknown", result.stderr, re.I):
                return None
            raise RuntimeError("registry inspection failed; publication stopped before further writes")
        return json.loads(result.stdout) if config else "sha256:" + sha256(result.stdout)

    def copy(self, archive, tag):
        command = ["skopeo", "copy", "--all", "--preserve-digests"]
        if self.test:
            command += ["--dest-tls-verify=false"]
        command += ["oci-archive:" + str(Path(archive).resolve()), self.reference(tag)]
        try:
            subprocess.run(command, check=True, timeout=600)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired):
            # The caller reconciles the tag once. Never repeat an uncertain push here.
            return False
        return True

    def alias_version(self, tag):
        config = self.inspect(tag, config=True)
        if config is None:
            return None
        labels = config.get("config", {}).get("Labels", {})
        if labels.get("org.opencontainers.image.source") != "https://github.com/opensourceops/agentctl":
            raise ValueError("existing stable alias is not a reviewed agentctl image")
        version = labels.get("org.opencontainers.image.version", "")
        version_parts(version)
        return version


def promote(bundle, directory, registry):
    version = bundle["identity"]["version"]
    planned = []
    # Check both immutable tags and all alias histories before writing either variant.
    for image in sorted(bundle["images"], key=lambda i: i["variant"]):
        variant = image["variant"]
        immutable = image_tags(version, variant)[0]
        existing = registry.inspect(immutable)
        if existing is not None and existing != image["manifestDigest"]:
            raise ValueError(f"immutable version {immutable} already has different content; do not overwrite it")
        numbers, prerelease = version_parts(version)
        suffix = "-ci" if variant == "tooling" else ""
        newest = registry.alias_version("ci" if suffix else "latest") if not prerelease else None
        minor = registry.alias_version(f"{numbers[0]}.{numbers[1]}{suffix}") if not prerelease else None
        tags = image_tags(version, variant, newest, minor)
        planned.append((image, tags, existing))
    results = []

    def copy_checked(image, tag):
        registry.copy(Path(directory) / image["file"], tag)
        if registry.inspect(tag) != image["manifestDigest"]:
            raise RuntimeError(f"publication of {tag} is incomplete or uncertain; inspect remote state before rerunning")
        results.append({"tag": tag, "manifestDigest": image["manifestDigest"], "variant": image["variant"]})

    # A failed architecture cannot reach this function: bundle verification requires all four.
    # A failed version push must not advance any stable alias.
    for image, tags, existing in planned:
        if existing is None:
            copy_checked(image, tags[0])
        else:
            results.append({"tag": tags[0], "manifestDigest": existing, "variant": image["variant"], "reused": True})
    for image, tags, _ in planned:
        for tag in tags[1:]:
            current_version = registry.alias_version(tag)
            if current_version and version_parts(current_version)[0] > version_parts(version)[0]:
                results.append({"tag": tag, "skipped": "newer release already published", "version": current_version})
                continue
            if registry.inspect(tag) == image["manifestDigest"]:
                results.append({"tag": tag, "manifestDigest": image["manifestDigest"], "variant": image["variant"], "reused": True})
            else:
                copy_checked(image, tag)
    return {"repository": registry.repository, "identity": bundle["identity"], "published": results,
            "images": bundle["images"], "packages": bundle["packages"]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True)
    parser.add_argument("--source", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--registry", default=PRODUCTION)
    parser.add_argument("--disposable", action="store_true")
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    release = identity(".", args.source, args.tag)
    bundle = verify(args.directory, release)
    if not args.disposable:
        if os.environ.get("GITHUB_EVENT_NAME") not in {"release", "workflow_dispatch"} or os.environ.get("GITHUB_REPOSITORY") != "opensourceops/agentctl":
            raise ValueError("production publication requires the trusted release workflow")
        # Trusted workflow fetches this record itself; models and PR jobs cannot supply it.
        record = json.loads(Path("published-release.json").read_text())
        if record.get("draft") is not False or record.get("tag_name") != args.tag or not record.get("published_at"):
            raise ValueError("publish the fully prepared GitHub release before promoting images")
        if bool(record.get("prerelease")) != release["prerelease"]:
            raise ValueError("GitHub prerelease status and version disagree")
    result = promote(bundle, args.directory, Registry(args.registry, args.disposable))
    result["releaseBundleSha256"] = file_digest(Path(args.directory) / "release-bundle.json")
    write_json(args.output, result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
