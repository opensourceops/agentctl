"""Release gating tests: synthetic transport artifacts, no external effects."""
import json
import os
from pathlib import Path
import shutil
from types import SimpleNamespace
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import artifacts
import bundle
from test_artifacts import IDENTITY, layout, write_layout


def write_json(path, value):
    Path(path).write_text(json.dumps(value, sort_keys=True) + "\n")


def cdx(name="agentctl", image=None):
    component = {"type": "application", "name": name, "version": "0.4.0"}
    if image:
        component = {"type": "container", "name": image, "hashes": [{"alg": "SHA-256", "content": image[7:]}]}
    return {"bomFormat": "CycloneDX", "specVersion": "1.5", "version": 1,
            "metadata": {"component": component},
            "components": [{"type": "library", "name": "fixture-dependency", "version": "1.0.0", "purl": "pkg:cargo/fixture-dependency@1.0.0"}]}


def hello_envelope():
    # Actual contract: CLI Envelope<T>, RunOutcome; archived real CLI evidence also
    # contains this shape. `result` is not a success channel in agentctl.dev/cli/v1.
    return {"apiVersion": "agentctl.dev/cli/v1", "kind": "RunOutcome", "ok": True,
            "data": {"runId": "run-fixture", "traceId": "trace-fixture", "state": "succeeded",
                     "output": {"greeting": "hello, world"}}, "diagnostics": []}


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.source = self.root / "source"
        self.source.mkdir()
        (self.source / "Cargo.toml").write_text('[workspace.package]\nversion = "0.4.0"\n')
        hello = self.source / "examples/v1/hello.yaml"
        hello.parent.mkdir(parents=True)
        hello.write_text((Path(__file__).resolve().parents[2] / "examples/v1/hello.yaml").read_text())
        self.incoming, self.output = self.root / "incoming", self.root / "prepared"
        self.incoming.mkdir()
        self.environment = patch.dict(os.environ, {"GITHUB_RUN_ID": "12345", "GITHUB_RUN_ATTEMPT": "1"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def native(self, target, envelope=None):
        directory = self.source / "dist" / f"agentctl-0.4.0-{target}"
        directory.mkdir(parents=True)
        binary = directory / ("agentctl.exe" if target.endswith("windows-msvc") else "agentctl")
        binary.write_bytes(b"synthetic native binary fixture " + target.encode())
        binary.chmod(0o755)
        for name in ["README.md", "LICENSE", "_agentctl", "_agentctl.ps1", "agentctl.bash", "agentctl.fish"]:
            (directory / name).write_text("fixture " + name + "\n")
        (directory / "SHA256SUMS").write_text(artifacts.file_digest(binary) + "  " + binary.name + "\n")
        sbom = self.root / (target + ".cdx.json")
        write_json(sbom, cdx())
        output = self.incoming / target
        args = SimpleNamespace(root=str(self.source), target=target, sbom=str(sbom), output=str(output))
        response = hello_envelope() if envelope is None else envelope
        with patch.object(bundle.subprocess, "check_output", return_value="agentctl 0.4.0\n") as version, \
             patch.object(bundle.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, json.dumps(response), "")) as execute:
            bundle.package(args, IDENTITY)
        self.assertEqual(Path(version.call_args.args[0][0]), binary.resolve())
        self.assertEqual(Path(execute.call_args.args[0][0]), binary.resolve())
        self.assertIn("--db", execute.call_args.args[0])
        return next(output.glob("*.record.json"))

    def image(self, variant, platform, passed=True, findings=None):
        arch = platform.split("/")[1]
        folder = self.incoming / (variant + "-" + arch)
        folder.mkdir()
        name = variant + "-" + arch
        files, image, _ = layout(architecture=arch, variant=variant)
        archive = write_layout(folder / (name + ".oci.tar"), files)
        config_digest = json.loads(files["blobs/sha256/" + image["digest"][7:]])["config"]["digest"]
        scan, sbom, smoke = [folder / (name + extension) for extension in [".trivy.json", ".cdx.json", ".smoke.json"]]
        write_json(scan, {"SchemaVersion": 2, "ArtifactType": "container_image", "Metadata": {"ImageID": config_digest},
            "Results": [{"Target": "fixture (debian 12)", "Class": "os-pkgs", "Type": "debian", "Vulnerabilities": findings or []}]})
        write_json(sbom, cdx(image=config_digest))
        write_json(smoke, {"imageId": config_digest, "platform": platform, "passed": passed})
        record = folder / (name + ".record.json")
        args = SimpleNamespace(archive=str(archive), variant=variant, platform=platform,
                               scan=str(scan), sbom=str(sbom), smoke=str(smoke), output=str(record))
        bundle.image(args, IDENTITY)
        return record

    def inputs(self):
        for target in sorted(artifacts.TARGETS):
            self.native(target)
        for variant in sorted(artifacts.VARIANTS):
            for platform in sorted(artifacts.PLATFORMS):
                self.image(variant, platform)

    def assemble(self):
        bundle.assemble(SimpleNamespace(incoming=str(self.incoming), output=str(self.output)), IDENTITY)
        return bundle.verify(self.output, IDENTITY)

    def update_bundle(self, mutate):
        path = self.output / "release-bundle.json"
        value = bundle.load(path)
        mutate(value)
        write_json(path, value)
        # Keep the outer checksum file consistent so negative cases exercise the
        # semantic contract rather than merely a changed bundle JSON checksum.
        (self.output / "SHA256SUMS").write_text("".join(f"{artifacts.file_digest(p)}  {p.name}\n"
            for p in sorted(self.output.iterdir()) if p.is_file() and p.name != "SHA256SUMS"))

    def test_checkout_identity_requires_exact_source_and_matching_version(self):
        with patch.object(bundle.subprocess, "check_output", return_value="a" * 40 + "\n"):
            self.assertEqual(bundle.identity(self.source, "a" * 40, "v0.4.0"), IDENTITY)
            with self.assertRaisesRegex(ValueError, "checkout does not match"):
                bundle.identity(self.source, "b" * 40, "v0.4.0")
            with self.assertRaisesRegex(ValueError, "tag/version mismatch"):
                bundle.identity(self.source, "a" * 40, "v0.4.1")

    def test_package_smoke_accepts_real_data_envelope_and_executes_packaged_binary(self):
        record = bundle.load(self.native("x86_64-unknown-linux-gnu"))
        self.assertEqual(record["identity"], IDENTITY)
        self.assertIn("packaged-hello", record["gates"])
        self.assertEqual(record["version"], "0.4.0")

    def test_package_version_mismatch_fails_before_smoke_or_archive(self):
        target = "x86_64-unknown-linux-gnu"
        self.native(target)
        record = next((self.incoming / target).glob("*.record.json"))
        record.unlink()
        args = SimpleNamespace(root=str(self.source), target=target,
                               sbom=str(self.root / (target + ".cdx.json")), output=str(self.root / "wrong-version"))
        with patch.object(bundle.subprocess, "check_output", return_value="agentctl 0.3.0\n"), \
             patch.object(bundle.subprocess, "run") as execute:
            with self.assertRaisesRegex(ValueError, "version does not match"):
                bundle.package(args, IDENTITY)
            execute.assert_not_called()
        self.assertFalse(list((self.root / "wrong-version").glob("*.record.json")))

    def test_package_smoke_rejects_error_truthy_flags_wrong_output_and_legacy_result(self):
        cases = [hello_envelope() for _ in range(4)]
        cases[0]["ok"] = False
        cases[1]["ok"] = "true"
        cases[2]["data"]["output"]["greeting"] = "wrong"
        cases[3] = {"result": {"state": "succeeded"}}
        for index, response in enumerate(cases):
            with self.subTest(index=index), self.assertRaisesRegex(ValueError, "hello"):
                self.native(sorted(artifacts.TARGETS)[index], response)

    def test_fixed_high_findings_wrong_scan_identity_and_malformed_sbom_block(self):
        with self.assertRaisesRegex(ValueError, "vulnerabilities block"):
            self.image("minimal", "linux/amd64", findings=[{"VulnerabilityID": "CVE-fixture", "Severity": "HIGH", "FixedVersion": "1.2.3"}])
        scan = next(self.incoming.rglob("*.trivy.json"))
        with self.assertRaisesRegex(ValueError, "bind the tested image"):
            bundle.scan(scan, "sha256:" + "b" * 64)
        invalid = self.root / "invalid.cdx.json"
        value = cdx()
        value["components"] = "not an SBOM component list"
        write_json(invalid, value)
        with self.assertRaisesRegex(ValueError, "SBOM"):
            bundle.sbom(invalid, binary=True)

    def test_smoke_requires_literal_success_not_truthy_strings(self):
        with self.assertRaisesRegex(ValueError, "evidence"):
            self.image("minimal", "linux/amd64", passed="false")

    def test_complete_four_package_four_native_image_bundle_verifies_exact_identities(self):
        self.inputs()
        result = self.assemble()
        self.assertEqual(result["identity"], IDENTITY)
        self.assertEqual(len(result["packages"]), 4)
        self.assertEqual(len(result["images"]), 2)
        self.assertEqual(result["preparationRunId"], "12345")
        for image in result["images"]:
            with artifacts.OciArchive(self.output / image["file"]) as archive:
                self.assertEqual(archive.index["manifests"][0]["digest"], image["manifestDigest"])
                self.assertEqual({item["platform"] for item in archive.images(IDENTITY, image["variant"])["images"]}, artifacts.PLATFORMS)

    def test_missing_native_package_or_platform_and_mixed_source_block_assembly(self):
        self.inputs()
        path = next(self.incoming.glob("x86_64-unknown-linux-gnu/*.record.json"))
        native = path.read_bytes()
        path.unlink()
        with self.assertRaisesRegex(ValueError, "four native CLI packages"):
            self.assemble()
        self.output.rmdir()
        path.write_bytes(native)
        image = next(self.incoming.glob("tooling-arm64/*.record.json"))
        image_data = image.read_bytes()
        image.unlink()
        with self.assertRaisesRegex(ValueError, "both image variants"):
            self.assemble()
        self.output.rmdir()
        image.write_bytes(image_data)
        data = bundle.load(path)
        data["identity"]["sourceSha"] = "b" * 40
        write_json(path, data)
        with self.assertRaisesRegex(ValueError, "mix source"):
            self.assemble()

    def test_package_and_oci_transport_tamper_block_before_bundle_eligibility(self):
        self.inputs()
        package = next(self.incoming.rglob("*.tar.gz"))
        original = package.read_bytes()
        package.write_bytes(original + b"tampered transport")
        with self.assertRaisesRegex(ValueError, "package transport checksum"):
            self.assemble()
        self.output.rmdir()
        package.write_bytes(original)
        image = next(self.incoming.rglob("*.oci.tar"))
        image.write_bytes(image.read_bytes() + b"tampered transport")
        with self.assertRaisesRegex(ValueError, "OCI transport checksum"):
            self.assemble()

    def test_smoke_result_cannot_change_after_native_validation(self):
        self.inputs()
        smoke = next(self.incoming.rglob("*.smoke.json"))
        value = bundle.load(smoke)
        value["passed"] = False
        write_json(smoke, value)
        with self.assertRaisesRegex(ValueError, "smoke|evidence|checksum|transport"):
            self.assemble()

    def test_valid_but_different_sbom_cannot_replace_validated_package_sbom(self):
        self.inputs()
        sbom = next(self.incoming.glob("x86_64-unknown-linux-gnu/*.cdx.json"))
        value = bundle.load(sbom)
        value["components"][0]["version"] = "9.9.9"
        write_json(sbom, value)
        with self.assertRaisesRegex(ValueError, "SBOM|sbom|checksum|transport"):
            self.assemble()

    def test_duplicate_incoming_names_cannot_shadow_another_platform_artifact(self):
        self.inputs()
        selected = next(self.incoming.rglob("*.cdx.json"))
        other = self.incoming / "extra"
        other.mkdir()
        shutil.copyfile(selected, other / selected.name)
        with self.assertRaisesRegex(ValueError, "ambiguous"):
            self.assemble()

    def test_verify_rejects_missing_package_even_if_omitted_from_asset_inventory(self):
        self.inputs()
        document = self.assemble()
        missing = document["packages"][0]["file"]
        (self.output / missing).unlink()
        self.update_bundle(lambda value: value.update(assets=[asset for asset in value["assets"] if asset["file"] != missing]))
        with self.assertRaisesRegex(ValueError, "package|asset|inventory|checksum"):
            bundle.verify(self.output, IDENTITY)

    def test_verify_rejects_image_reference_outside_hashed_asset_inventory(self):
        self.inputs()
        document = self.assemble()
        shutil.copyfile(self.output / document["images"][0]["file"], self.root / "outside.oci.tar")
        self.update_bundle(lambda value: value["images"][0].update(file="../outside.oci.tar"))
        with self.assertRaisesRegex(ValueError, "unsafe|asset|inventory|reference"):
            bundle.verify(self.output, IDENTITY)

    def test_verify_rejects_tag_source_registry_or_platform_metadata_substitution(self):
        self.inputs()
        document = self.assemble()
        for release in [{**IDENTITY, "sourceSha": "b" * 40}, {**IDENTITY, "tag": "v0.4.1"}]:
            with self.subTest(release=release), self.assertRaisesRegex(ValueError, "tag/source/registry"):
                bundle.verify(self.output, release)
        self.update_bundle(lambda value: value.update(registry="docker.io/untrusted/agentctl"))
        with self.assertRaisesRegex(ValueError, "tag/source/registry"):
            bundle.verify(self.output, IDENTITY)
        write_json(self.output / "release-bundle.json", document)
        self.update_bundle(lambda value: value["images"][0].update(manifestDigest="sha256:" + "b" * 64))
        with self.assertRaisesRegex(ValueError, "index identity"):
            bundle.verify(self.output, IDENTITY)

    def test_verify_requires_complete_recorded_package_gate_inventory(self):
        self.inputs()
        self.assemble()
        self.update_bundle(lambda value: value["packages"][0]["gates"].remove("verify"))
        with self.assertRaisesRegex(ValueError, "gate inventory"):
            bundle.verify(self.output, IDENTITY)

    def test_consistent_replacement_evidence_cannot_detach_native_tests_from_actual_oci(self):
        self.inputs()
        self.assemble()

        def detach(value):
            combined = value["images"][0]
            platform = combined["platforms"][0]
            forged_config = "sha256:" + "b" * 64
            platform["configDigest"] = forged_config
            native = next(item for item in value["nativeImages"]
                          if item["variant"] == combined["variant"] and item["platform"] == platform["platform"])
            native["image"]["configDigest"] = forged_config
            for field in ["scan", "smoke"]:
                path = self.output / native[field]
                data = bundle.load(path)
                if field == "scan":
                    data["Metadata"]["ImageID"] = forged_config
                else:
                    data["imageId"] = forged_config
                write_json(path, data)
                native[field + "Sha256"] = artifacts.file_digest(path)
                asset = next(item for item in value["assets"] if item["file"] == path.name)
                asset.update(sha256=artifacts.file_digest(path), bytes=path.stat().st_size)

        self.update_bundle(detach)
        with self.assertRaisesRegex(ValueError, "platform|config|manifest|native"):
            bundle.verify(self.output, IDENTITY)


if __name__ == "__main__":
    unittest.main()
