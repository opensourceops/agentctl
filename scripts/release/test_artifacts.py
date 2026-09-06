"""Credential-free release contract tests using explicitly synthetic OCI blobs."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile


SPEC = importlib.util.spec_from_file_location("release_artifacts", Path(__file__).with_name("artifacts.py"))
artifacts = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(artifacts)

IDENTITY = {"version": "0.4.0", "tag": "v0.4.0", "sourceSha": "a" * 40, "prerelease": False}
MANIFEST = "application/vnd.oci.image.manifest.v1+json"
INDEX = "application/vnd.oci.image.index.v1+json"
CONFIG = "application/vnd.oci.image.config.v1+json"
STATEMENT = "https://in-toto.io/Statement/v0.1"
PROVENANCE = "https://slsa.dev/provenance/v0.2"


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def layout(architecture="amd64", variant="minimal", source=None, subject=None, annotation=None, user="65532:65532", oci_artifact=False):
    """Describe synthetic content-addressed blobs, never a runnable image."""
    files = {"oci-layout": b'{"imageLayoutVersion":"1.0.0"}'}

    def blob(value, media_type):
        content = value if isinstance(value, bytes) else encoded(value)
        digest = hashlib.sha256(content).hexdigest()
        files["blobs/sha256/" + digest] = content
        return {"mediaType": media_type, "digest": "sha256:" + digest, "size": len(content)}

    labels = {"org.opencontainers.image.version": IDENTITY["version"],
        "org.opencontainers.image.revision": source or IDENTITY["sourceSha"],
        "org.opencontainers.image.source": "https://github.com/opensourceops/agentctl",
        "org.opencontainers.image.licenses": "Apache-2.0", "dev.agentctl.variant": variant}
    config = blob({"os": "linux", "architecture": architecture,
        "config": {"Labels": labels, "User": user, "Entrypoint": ["/usr/local/bin/agentctl"], "WorkingDir": "/workspace"}}, CONFIG)
    layer = blob(b"synthetic layer contract fixture " + architecture.encode(), "application/vnd.oci.image.layer.v1.tar+gzip")
    image = blob({"schemaVersion": 2, "mediaType": MANIFEST, "config": config, "layers": [layer]}, MANIFEST)
    image["platform"] = {"os": "linux", "architecture": architecture}
    predicate = "https://slsa.dev/provenance/v1" if oci_artifact else PROVENANCE
    statement = blob({"_type": "https://in-toto.io/Statement/v1" if oci_artifact else STATEMENT, "predicateType": predicate,
        "subject": [{"name": "fixture", "digest": {"sha256": (subject or image["digest"])[7:]}}],
        "predicate": {"buildType": "https://mobyproject.org/buildkit@v1", "materials": []}}, "application/vnd.in-toto+json")
    statement["annotations"] = {"in-toto.io/predicate-type": predicate}
    if oci_artifact:
        # BuildKit v0.32.2 exporter/containerimage/writer.go:578-608, 631-642.
        # Exact published OCI empty descriptor; synthetic provenance body above.
        attestation_config = blob(b"{}", "application/vnd.oci.empty.v1+json")
        attestation_config["data"] = "e30="
    else:
        attestation_config = blob({"os": "unknown", "architecture": "unknown"}, CONFIG)
    manifest = {"schemaVersion": 2, "mediaType": MANIFEST, "config": attestation_config, "layers": [statement]}
    if oci_artifact:
        manifest.update(artifactType="application/vnd.docker.attestation.manifest.v1+json",
                        subject={key: image[key] for key in ["mediaType", "digest", "size"]})
    attestation = blob(manifest, MANIFEST)
    attestation["platform"] = {"os": "unknown", "architecture": "unknown"}
    attestation["annotations"] = {"vnd.docker.reference.type": "attestation-manifest",
        "vnd.docker.reference.digest": annotation or image["digest"]}
    files["index.json"] = encoded({"schemaVersion": 2, "mediaType": INDEX, "manifests": [image, attestation]})
    return files, image, layer


def write_layout(path, files, extras=()):
    with tarfile.open(path, "x") as archive:
        for name, data in [*files.items(), *extras]:
            member = tarfile.TarInfo(name)
            if data is None:
                member.type = tarfile.SYMTYPE
                member.linkname = "/outside"
                archive.addfile(member)
            else:
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
    return path


class ReleaseIdentityTests(unittest.TestCase):
    def test_tag_version_and_source_identity_are_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "0.4.0"\n')
            self.assertEqual(artifacts.release_identity(root, "a" * 40, "v0.4.0"), IDENTITY)
            with self.assertRaisesRegex(ValueError, "tag/version mismatch"):
                artifacts.release_identity(root, "a" * 40, "v0.3.0")
            for source in ["main", "A" * 40, "a" * 39, "g" * 40]:
                with self.subTest(source=source), self.assertRaisesRegex(ValueError, "exact lowercase"):
                    artifacts.release_identity(root, source, "v0.4.0")

    def test_prerelease_and_older_release_do_not_move_stable_aliases(self):
        self.assertEqual(artifacts.image_tags("0.4.0-rc.1", "minimal"), ["0.4.0-rc.1"])
        self.assertEqual(artifacts.image_tags("0.4.0-rc.1", "tooling"), ["0.4.0-rc.1-ci"])
        self.assertEqual(artifacts.image_tags("0.4.1", "minimal", "0.5.0", "0.4.2"), ["0.4.1"])
        self.assertEqual(artifacts.image_tags("0.4.3", "tooling", "0.5.0", "0.4.2"), ["0.4.3-ci", "0.4-ci"])
        self.assertEqual(artifacts.image_tags("0.5.1", "minimal", "0.5.0", "0.5.0"), ["0.5.1", "0.5", "latest"])
        self.assertEqual(artifacts.image_tags("0.5.1", "tooling", "0.5.0", "0.5.0"), ["0.5.1-ci", "0.5-ci", "ci"])

    def test_invalid_semver_or_prerelease_alias_history_is_rejected(self):
        for version in ["0.04.0", "v0.4.0", "0.4", "0.4.0+build", "0.4.0-01"]:
            with self.subTest(version=version), self.assertRaises(ValueError):
                artifacts.version_parts(version)
        with self.assertRaisesRegex(ValueError, "cannot refer to a prerelease"):
            artifacts.image_tags("0.4.0", "minimal", "0.5.0-rc.1")


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def package(self, target):
        root = self.root / f"agentctl-0.4.0-{target}"
        root.mkdir()
        binary = "agentctl.exe" if target.endswith("windows-msvc") else "agentctl"
        (root / binary).write_bytes(b"synthetic binary fixture")
        if binary == "agentctl":
            (root / binary).chmod(0o755)
        for name in ["README.md", "LICENSE", "_agentctl", "_agentctl.ps1", "agentctl.bash", "agentctl.fish"]:
            (root / name).write_text("fixture " + name + "\n")
        (root / "SHA256SUMS").write_text(artifacts.file_digest(root / binary) + "  " + binary + "\n")
        return root

    def test_native_archives_preserve_inventory_binary_bytes_and_download_digest(self):
        for target in sorted(artifacts.TARGETS):
            with self.subTest(target=target):
                package = self.package(target)
                output = self.root / (target + (".zip" if target.endswith("windows-msvc") else ".tar.gz"))
                result = artifacts.archive_package(package, target, "0.4.0", output)
                self.assertEqual(result["sha256"], hashlib.sha256(output.read_bytes()).hexdigest())
                self.assertEqual(result["bytes"], output.stat().st_size)
                expected = {f"{package.name}/{path.name}": path.read_bytes() for path in package.iterdir()}
                if output.suffix == ".zip":
                    with zipfile.ZipFile(output) as archive:
                        actual = {name: archive.read(name) for name in archive.namelist()}
                else:
                    with tarfile.open(output) as archive:
                        actual = {member.name: archive.extractfile(member).read() for member in archive if member.isfile()}
                        self.assertEqual(archive.getmember(package.name + "/agentctl").mode & 0o111,
                                         (package / "agentctl").stat().st_mode & 0o111)
                self.assertEqual(actual, expected)
                with self.assertRaisesRegex(ValueError, "overwrite"):
                    artifacts.archive_package(package, target, "0.4.0", output)

    def test_tampered_checksum_extra_missing_and_nonregular_package_members_fail(self):
        package = self.package("x86_64-unknown-linux-gnu")
        output = self.root / "bad.tar.gz"
        (package / "agentctl").write_bytes(b"tampered binary")
        with self.assertRaisesRegex(ValueError, "SHA256SUMS"):
            artifacts.archive_package(package, "x86_64-unknown-linux-gnu", "0.4.0", output)
        (package / "SHA256SUMS").write_text(artifacts.file_digest(package / "agentctl") + "  agentctl\n")
        (package / "extra").write_text("unexpected")
        with self.assertRaisesRegex(ValueError, "missing, extra or non-regular"):
            artifacts.archive_package(package, "x86_64-unknown-linux-gnu", "0.4.0", output)
        (package / "extra").unlink()
        (package / "README.md").unlink()
        with self.assertRaisesRegex(ValueError, "missing, extra or non-regular"):
            artifacts.archive_package(package, "x86_64-unknown-linux-gnu", "0.4.0", output)
        (package / "README.md").mkdir()
        with self.assertRaisesRegex(ValueError, "missing, extra or non-regular"):
            artifacts.archive_package(package, "x86_64-unknown-linux-gnu", "0.4.0", output)
        self.assertFalse(output.exists())

    def test_package_directory_cannot_be_relabeled_to_another_version_or_target(self):
        package = self.package("x86_64-unknown-linux-gnu")
        for target, version in [("x86_64-unknown-linux-gnu", "0.5.0"), ("aarch64-unknown-linux-gnu", "0.4.0")]:
            with self.subTest(target=target, version=version), self.assertRaisesRegex(ValueError, "does not match"):
                artifacts.archive_package(package, target, version, self.root / "bad.tar.gz")


class OciTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)

    def image(self, name="fixture.tar", **options):
        files, descriptor, layer = layout(**options)
        return write_layout(self.root / name, files), descriptor, layer

    def test_source_variant_nonroot_and_platform_are_bound_to_image_config(self):
        path, descriptor, _ = self.image()
        with artifacts.OciArchive(path) as archive:
            result = archive.images(IDENTITY, "minimal")
            self.assertEqual(result["images"][0]["platform"], "linux/amd64")
            self.assertEqual(result["images"][0]["descriptor"]["digest"], descriptor["digest"])
            self.assertEqual(len(result["attestations"]), 1)
            with self.assertRaisesRegex(ValueError, "labels"):
                archive.images({**IDENTITY, "sourceSha": "b" * 40}, "minimal")
            with self.assertRaisesRegex(ValueError, "labels"):
                archive.images(IDENTITY, "tooling")
        for name, changes, expected in [("user", {"user": "0"}, "non-root"),
                                         ("platform", {"architecture": "riscv64"}, "platform")]:
            with self.subTest(name=name):
                path, _, _ = self.image(name + ".tar", **changes)
                with artifacts.OciArchive(path) as archive, self.assertRaisesRegex(ValueError, expected):
                    archive.images(IDENTITY, "minimal")

    def test_corrupt_blob_missing_layer_duplicate_paths_and_escape_never_extract(self):
        files, _, layer = layout()
        corrupted = dict(files)
        corrupted["blobs/sha256/" + layer["digest"][7:]] += b"tampered"
        bad = write_layout(self.root / "corrupt.tar", corrupted)
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            artifacts.OciArchive(bad)
        missing = dict(files)
        del missing["blobs/sha256/" + layer["digest"][7:]]
        bad = write_layout(self.root / "missing.tar", missing)
        with artifacts.OciArchive(bad) as archive, self.assertRaisesRegex(ValueError, "missing image layer"):
            archive.images(IDENTITY, "minimal")
        for number, name, data in [(1, "./index.json", files["index.json"]), (2, "../outside", b"escape"),
                                   (3, "/absolute", b"escape"), (4, "blobs/link", None), (5, "blobs\\escape", b"escape")]:
            with self.subTest(name=name):
                bad = write_layout(self.root / f"unsafe-{number}.tar", files, [(name, data)])
                with self.assertRaises(ValueError):
                    artifacts.OciArchive(bad)
        self.assertFalse((self.root.parent / "outside").exists())

    def test_duplicate_runnable_platform_cannot_form_multiarch_release(self):
        left, _, _ = self.image("left.tar")
        right, _, _ = self.image("right.tar")
        with self.assertRaisesRegex(ValueError, "both native runnable platforms"):
            artifacts.merge_oci_archives([left, right], self.root / "bad.tar", IDENTITY, "minimal")

    def test_attestation_annotation_and_statement_subject_must_bind_same_manifest(self):
        for name, changes in [("annotation", {"annotation": "sha256:" + "b" * 64}),
                              ("subject", {"subject": "sha256:" + "b" * 64})]:
            with self.subTest(name=name):
                path, _, _ = self.image(name + ".tar", **changes)
                with artifacts.OciArchive(path) as archive, self.assertRaisesRegex(ValueError, "provenance|attestation|subject"):
                    archive.images(IDENTITY, "minimal")

    def test_buildkit_032_empty_config_artifact_retains_nonrunnable_platform_and_subject_binding(self):
        files, image, _ = layout(oci_artifact=True)
        path = write_layout(self.root / "buildkit-032.tar", files)
        with artifacts.OciArchive(path) as archive:
            result = archive.images(IDENTITY, "minimal")
            self.assertEqual(len(result["images"]), 1)
            self.assertEqual(len(result["attestations"]), 1)
            statement = archive.descriptor(result["attestations"][0])
            self.assertEqual(statement["subject"]["digest"], image["digest"])
            self.assertEqual(archive.descriptor(statement["config"]), {})

    def test_modern_attestation_cannot_hide_runnable_config_or_wrong_artifact_subject(self):
        for case in ["platform", "artifactType", "config", "inlineData", "subject", "subjectSize", "missingSubject"]:
            with self.subTest(case=case):
                files, image, _ = layout(oci_artifact=True)
                index = json.loads(files["index.json"])
                descriptor = index["manifests"][-1]
                manifest = json.loads(files["blobs/sha256/" + descriptor["digest"][7:]])
                if case == "platform": descriptor["platform"] = {"os": "linux", "architecture": "amd64"}
                elif case == "artifactType": manifest["artifactType"] = "application/unreviewed"
                elif case == "config": manifest["config"] = json.loads(files["blobs/sha256/" + image["digest"][7:]])["config"]
                elif case == "inlineData": manifest["config"]["data"] = "bnVsbA=="
                elif case == "subject": manifest["subject"]["digest"] = "sha256:" + "b" * 64
                elif case == "subjectSize": manifest["subject"]["size"] += 1
                else: del manifest["subject"]
                payload = encoded(manifest)
                descriptor.update(digest="sha256:" + artifacts.sha256(payload), size=len(payload))
                files["blobs/sha256/" + descriptor["digest"][7:]] = payload
                files["index.json"] = encoded(index)
                path = write_layout(self.root / (case + ".tar"), files)
                with artifacts.OciArchive(path) as archive, self.assertRaises(ValueError):
                    archive.images(IDENTITY, "minimal")

    def test_untagged_buildkit_export_with_empty_statement_subject_cannot_claim_provenance(self):
        # Observed in hosted run34051210706: a valid OCI artifact subject alone
        # does not repair the empty SLSA v1 statement emitted by an untagged build.
        files, _, _ = layout(oci_artifact=True)
        index = json.loads(files["index.json"])
        descriptor = index["manifests"][-1]
        manifest = json.loads(files["blobs/sha256/" + descriptor["digest"][7:]])
        layer = manifest["layers"][0]
        statement = json.loads(files["blobs/sha256/" + layer["digest"][7:]])
        statement["subject"] = []
        for value, target in [(statement, layer), (manifest, descriptor)]:
            payload = encoded(value)
            target.update(digest="sha256:" + artifacts.sha256(payload), size=len(payload))
            files["blobs/sha256/" + target["digest"][7:]] = payload
        files["index.json"] = encoded(index)
        path = write_layout(self.root / "untagged-buildkit.tar", files)
        with artifacts.OciArchive(path) as archive, self.assertRaisesRegex(ValueError, "provenance subject"):
            archive.images(IDENTITY, "minimal")

    def test_merge_preserves_exact_platform_manifests_layers_and_provenance(self):
        left, left_image, left_layer = self.image("amd64.tar")
        right, right_image, right_layer = self.image("arm64.tar", architecture="arm64", oci_artifact=True)
        output = self.root / "combined.tar"
        result = artifacts.merge_oci_archives([left, right], output, IDENTITY, "minimal")
        self.assertEqual(result["sha256"], artifacts.file_digest(output))
        self.assertEqual({item["manifestDigest"] for item in result["platforms"]}, {left_image["digest"], right_image["digest"]})
        self.assertEqual(len(result["provenanceDescriptors"]), 2)
        with artifacts.OciArchive(output) as archive:
            self.assertEqual(archive.index["manifests"][0]["digest"], result["manifestDigest"])
            self.assertEqual({item["platform"] for item in archive.images(IDENTITY, "minimal")["images"]}, artifacts.PLATFORMS)
            for layer in [left_layer, right_layer]:
                self.assertIn("blobs/sha256/" + layer["digest"][7:], archive.members)

    def test_reassembling_complete_layout_never_emits_duplicate_root_blob(self):
        left, _, _ = self.image("amd64.tar")
        right, _, _ = self.image("arm64.tar", architecture="arm64")
        first, second = self.root / "combined.tar", self.root / "reassembled.tar"
        initial = artifacts.merge_oci_archives([left, right], first, IDENTITY, "minimal")
        repeated = artifacts.merge_oci_archives([first], second, IDENTITY, "minimal")
        self.assertEqual(repeated["manifestDigest"], initial["manifestDigest"])
        with artifacts.OciArchive(second) as archive:
            self.assertEqual(len(archive.images(IDENTITY, "minimal")["images"]), 2)


if __name__ == "__main__":
    unittest.main()
