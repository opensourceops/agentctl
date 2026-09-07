"""Release identities and OCI transport validation; no registry or GitHub writes."""
from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import tarfile
import tomllib
import zipfile

SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z")
TARGETS = {"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-pc-windows-msvc"}
PLATFORMS = {"linux/amd64", "linux/arm64"}
VARIANTS = {"minimal", "tooling"}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def version_parts(value):
    match = VERSION.fullmatch(value)
    if not match:
        raise ValueError("release version must be SemVer without build metadata")
    if value.endswith("-ci"):
        raise ValueError("release versions ending in -ci collide with the reserved tooling image suffix")
    prerelease = match[4].split(".") if match[4] else []
    if any(part.isdigit() and len(part) > 1 and part.startswith("0") for part in prerelease):
        raise ValueError("numeric prerelease identifiers cannot have leading zeroes")
    return tuple(map(int, match.group(1, 2, 3))), prerelease


def release_identity(root, source_sha, tag):
    if not SHA.fullmatch(source_sha):
        raise ValueError("release source must be an exact lowercase 40-character commit SHA")
    version = tomllib.loads((Path(root) / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    _, prerelease = version_parts(version)
    if tag != "v" + version:
        raise ValueError(f"tag/version mismatch: expected v{version}; prepare assets from the matching reviewed source")
    return {"version": version, "tag": tag, "sourceSha": source_sha, "prerelease": bool(prerelease)}


def image_tags(version, variant, newest_stable=None, newest_minor=None):
    numbers, prerelease = version_parts(version)
    if variant not in VARIANTS:
        raise ValueError("unknown image variant")
    suffix = "-ci" if variant == "tooling" else ""
    tags = [version + suffix]
    if prerelease:
        return tags
    for previous in [newest_stable, newest_minor]:
        if previous and version_parts(previous)[1]:
            raise ValueError("stable alias history cannot refer to a prerelease")
    if newest_minor is None or numbers >= version_parts(newest_minor)[0]:
        tags.append(f"{numbers[0]}.{numbers[1]}{suffix}")
    if newest_stable is None or numbers >= version_parts(newest_stable)[0]:
        tags.append("ci" if variant == "tooling" else "latest")
    return tags


def safe_member(name):
    path = PurePosixPath(name)
    if not name or path.is_absolute() or ".." in path.parts or "\\" in name:
        raise ValueError(f"unsafe archive member: {name!r}")
    return str(path)


def archive_package(directory, target, version, output):
    """Preserve existing package contents/checksums and create the download archive."""
    if target not in TARGETS:
        raise ValueError("unsupported native package target")
    version_parts(version)
    directory, output = Path(directory), Path(output)
    if directory.name != f"agentctl-{version}-{target}":
        raise ValueError("package directory does not match version and target")
    binary = "agentctl.exe" if target.endswith("windows-msvc") else "agentctl"
    required = {binary, "README.md", "LICENSE", "SHA256SUMS", "_agentctl", "_agentctl.ps1", "agentctl.bash", "agentctl.fish"}
    if {p.name for p in directory.iterdir()} != required or any(not p.is_file() or p.is_symlink() for p in directory.iterdir()):
        raise ValueError("package has missing, extra or non-regular contents")
    checksum = directory.joinpath("SHA256SUMS").read_text().strip().split()
    if checksum != [file_digest(directory / binary), binary]:
        raise ValueError("package binary does not match its SHA256SUMS")
    if output.exists():
        raise ValueError("refuse to overwrite an existing release archive")
    output.parent.mkdir(parents=True, exist_ok=True)
    if target.endswith("windows-msvc"):
        with zipfile.ZipFile(output, "x", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(directory.iterdir()):
                archive.write(path, arcname=f"{directory.name}/{path.name}")
    else:
        with tarfile.open(output, "x:gz") as archive:
            archive.add(directory, arcname=directory.name, recursive=True)
    return {"file": output.name, "target": target, "version": version,
            "sha256": file_digest(output), "bytes": output.stat().st_size,
            "binarySha256": checksum[0]}


class OciArchive:
    """Read an OCI layout without extracting untrusted paths into the filesystem."""
    def __init__(self, filename):
        self.filename = Path(filename)
        self.archive = tarfile.open(filename, "r:*")
        try:
            self.members = {}
            for item in self.archive:
                name = safe_member(item.name)
                if item.isdir():
                    continue
                if not item.isfile() or name in self.members:
                    raise ValueError("OCI archive has duplicate or non-regular members")
                if name not in {"index.json", "oci-layout"} and not re.fullmatch(r"blobs/sha256/[0-9a-f]{64}", name):
                    raise ValueError("OCI archive has an unexpected member")
                self.members[name] = item
                if name.startswith("blobs/"):
                    stream = self.archive.extractfile(item)
                    if hashlib.file_digest(stream, "sha256").hexdigest() != name.rsplit("/", 1)[1]:
                        raise ValueError("OCI blob digest mismatch")
            if self.read_json("oci-layout") != {"imageLayoutVersion": "1.0.0"}:
                raise ValueError("unsupported OCI layout")
            self.index = self.read_json("index.json")
            if self.index.get("schemaVersion") != 2 or not isinstance(self.index.get("manifests"), list):
                raise ValueError("invalid OCI index")
        except BaseException:
            self.archive.close()
            raise

    def close(self):
        self.archive.close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()

    def read_json(self, name):
        member = self.members.get(name)
        if member is None or member.size > 16 * 1024 * 1024:
            raise ValueError("missing or oversized OCI metadata")
        return json.load(self.archive.extractfile(member))

    def descriptor(self, descriptor):
        digest = descriptor.get("digest", "")
        if not DIGEST.fullmatch(digest):
            raise ValueError("invalid descriptor digest")
        name = "blobs/sha256/" + digest[7:]
        if name not in self.members or self.members[name].size != descriptor.get("size"):
            raise ValueError("missing OCI descriptor or incorrect size")
        return self.read_json(name)

    def images(self, identity, variant):
        """Validate image configs and retain attached provenance descriptors separately."""
        if variant not in VARIANTS:
            raise ValueError("unknown image variant")
        images, attestations, seen = [], [], set()

        def visit(descriptor, depth=0):
            if depth > 4 or descriptor["digest"] in seen:
                raise ValueError("recursive or duplicate OCI descriptor")
            seen.add(descriptor["digest"])
            document = self.descriptor(descriptor)
            if "manifests" in document:
                for child in document["manifests"]:
                    visit(child, depth + 1)
                return
            annotations = descriptor.get("annotations", {})
            if annotations.get("vnd.docker.reference.type") == "attestation-manifest":
                platform = descriptor.get("platform", {})
                if platform.get("os") != "unknown" or platform.get("architecture") != "unknown":
                    raise ValueError("attestation descriptor claims a runnable platform")
                config = self.descriptor(document["config"])
                if "artifactType" in document:
                    # BuildKit v0.32 uses OCI artifacts: the non-runnable platform
                    # belongs to the index descriptor, and config is exactly {}.
                    # https://docs.docker.com/build/metadata/attestations/attestation-storage/
                    empty = document["config"]
                    if (document["artifactType"] != "application/vnd.docker.attestation.manifest.v1+json"
                            or empty.get("mediaType") != "application/vnd.oci.empty.v1+json"
                            or empty.get("digest") != "sha256:" + sha256(b"{}")
                            or empty.get("size") != 2 or empty.get("data", "e30=") != "e30=" or config != {}):
                        raise ValueError("invalid OCI attestation artifact or empty config")
                    subject = document.get("subject", {})
                    if (subject.get("digest") != annotations.get("vnd.docker.reference.digest")
                            or subject.get("mediaType") != "application/vnd.oci.image.manifest.v1+json"):
                        raise ValueError("OCI attestation subject differs from its runnable manifest")
                    self.descriptor(subject)  # Validate the referenced bytes and size too.
                elif (config.get("os") != "unknown" or config.get("architecture") != "unknown"
                      or document["config"].get("mediaType") != "application/vnd.oci.image.config.v1+json"):
                    raise ValueError("legacy attestation config claims a runnable platform")
                layers = document.get("layers", [])
                if not layers:
                    raise ValueError("attestation manifest has no provenance statements")
                for layer in layers:
                    statement = self.descriptor(layer)
                    predicate = statement.get("predicateType")
                    if layer.get("mediaType") != "application/vnd.in-toto+json" or predicate not in {
                        "https://slsa.dev/provenance/v0.2", "https://slsa.dev/provenance/v1"
                    } or layer.get("annotations", {}).get("in-toto.io/predicate-type") != predicate:
                        raise ValueError("unsupported or mislabeled provenance statement")
                    subjects = statement.get("subject", [])
                    expected_digest = annotations.get("vnd.docker.reference.digest", "")
                    if not DIGEST.fullmatch(expected_digest) or not subjects or any(
                        subject.get("digest") != {"sha256": expected_digest[7:]} for subject in subjects
                    ):
                        raise ValueError("provenance subject does not bind its runnable manifest")
                attestations.append(descriptor)
                return
            config = self.descriptor(document["config"])
            platform = f"{config.get('os')}/{config.get('architecture')}"
            if platform not in PLATFORMS:
                raise ValueError("unexpected runnable platform")
            for layer in document.get("layers", []):
                digest = layer.get("digest", "")
                name = "blobs/sha256/" + digest.removeprefix("sha256:")
                if not DIGEST.fullmatch(digest) or name not in self.members or self.members[name].size != layer.get("size"):
                    raise ValueError("missing image layer or incorrect layer size")
            runtime = config.get("config", {})
            labels = runtime.get("Labels", {})
            expected = {"org.opencontainers.image.version": identity["version"],
                        "org.opencontainers.image.revision": identity["sourceSha"],
                        "org.opencontainers.image.source": "https://github.com/opensourceops/agentctl",
                        "org.opencontainers.image.licenses": "Apache-2.0", "dev.agentctl.variant": variant}
            if any(labels.get(key) != value for key, value in expected.items()):
                raise ValueError("image labels do not bind the release identity")
            if runtime.get("User") not in {"65532:65532", "nonroot:nonroot"}:
                raise ValueError("image must retain the reviewed non-root identity")
            if runtime.get("Entrypoint") != ["/usr/local/bin/agentctl"] or runtime.get("WorkingDir") != "/workspace":
                raise ValueError("image entrypoint/workspace contract mismatch")
            images.append({"platform": platform, "descriptor": descriptor, "configDigest": document["config"]["digest"]})

        for descriptor in self.index["manifests"]:
            visit(descriptor)
        if not images or len({image["platform"] for image in images}) != len(images):
            raise ValueError("missing or duplicate runnable image platforms")
        runnable = {image["descriptor"]["digest"] for image in images}
        if any(item.get("annotations", {}).get("vnd.docker.reference.digest") not in runnable for item in attestations):
            raise ValueError("provenance does not bind a runnable manifest")
        return {"images": images, "attestations": attestations}


def merge_oci_archives(archives, output, identity, variant):
    """Assemble exact verified manifests/blobs; never rebuild or transform them."""
    output = Path(output)
    if output.exists():
        raise ValueError("refuse to overwrite an existing OCI bundle")
    opened = []
    try:
        images, attestations, blobs = [], [], {}
        for filename in archives:
            archive = OciArchive(filename)
            opened.append(archive)
            verified = archive.images(identity, variant)
            images.extend(verified["images"])
            attestations.extend(verified["attestations"])
            for name, member in archive.members.items():
                if name.startswith("blobs/"):
                    blobs.setdefault(name, (archive, member))
        if len(images) != 2 or {image["platform"] for image in images} != PLATFORMS:
            raise ValueError("both native runnable platforms are required before assembly")
        descriptors = [dict(image["descriptor"], platform={"os": "linux", "architecture": image["platform"].split("/")[1]}) for image in sorted(images, key=lambda item: item["platform"])]
        index = {"schemaVersion": 2, "mediaType": "application/vnd.oci.image.index.v1+json", "manifests": descriptors + attestations}
        payload = json.dumps(index, separators=(",", ":"), sort_keys=True).encode()
        root_descriptor = {"mediaType": index["mediaType"], "digest": "sha256:" + sha256(payload), "size": len(payload)}
        layout_index = json.dumps({"schemaVersion": 2, "manifests": [root_descriptor]}, separators=(",", ":")).encode()
        with tarfile.open(output, "x") as target:
            # Skopeo can unpack explicit root-owned directories without chown,
            # but its implicit parent-directory creation tries to chown them.
            # Emit parents first so the archive works for unprivileged users.
            for name in ["blobs", "blobs/sha256"]:
                member = tarfile.TarInfo(name)
                member.type, member.mode = tarfile.DIRTYPE, 0o755
                target.addfile(member)
            for name, data in [("oci-layout", b'{"imageLayoutVersion":"1.0.0"}'), ("index.json", layout_index), ("blobs/sha256/" + sha256(payload), payload)]:
                member = tarfile.TarInfo(name)
                member.size, member.mode = len(data), 0o644
                target.addfile(member, io.BytesIO(data))
            for name, (archive, old) in sorted(blobs.items()):
                if name == "blobs/sha256/" + sha256(payload):
                    continue
                member = tarfile.TarInfo(name)
                member.size, member.mode = old.size, 0o644
                target.addfile(member, archive.archive.extractfile(old))
        return {"file": output.name, "sha256": file_digest(output), "manifestDigest": root_descriptor["digest"], "variant": variant,
                "platforms": [{"platform": image["platform"], "manifestDigest": image["descriptor"]["digest"], "configDigest": image["configDigest"]} for image in images],
                "provenanceDescriptors": [item["digest"] for item in attestations]}
    finally:
        for archive in opened:
            archive.close()
