from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

from publish import Registry, promote
from artifacts import image_tags


class MemoryRegistry:
    repository = "localhost:5000/agentctl"

    def __init__(self):
        self.tags = {}
        self.versions = {}
        self.writes = []
        self.fail_tag = None
        self.uncertain_tag = None

    def inspect(self, tag, config=False):
        return self.tags.get(tag)

    def alias_version(self, tag):
        return self.versions.get(tag)

    def copy(self, path, tag):
        self.writes.append(tag)
        if tag == self.fail_tag:
            return False
        variant = "tooling" if "tooling" in str(path) else "minimal"
        self.tags[tag] = "sha256:" + ("b" if variant == "tooling" else "a") * 64
        return tag != self.uncertain_tag


def bundle(version="0.4.0"):
    return {"identity": {"version": version}, "packages": [], "images": [
        {"variant": "minimal", "file": "minimal.oci.tar", "manifestDigest": "sha256:" + "a" * 64},
        {"variant": "tooling", "file": "tooling.oci.tar", "manifestDigest": "sha256:" + "b" * 64}]}


class PromotionTests(unittest.TestCase):
    def test_prerelease_cannot_occupy_another_releases_tooling_tag(self):
        for version in ["0.4.0-ci", "0.4.0-rc.1-ci"]:
            with self.assertRaisesRegex(ValueError, "tooling"):
                image_tags(version, "minimal")

    def test_version_tags_are_verified_before_stable_aliases_and_rerun_is_read_only(self):
        registry = MemoryRegistry()
        promote(bundle(), Path("."), registry)
        self.assertEqual(registry.writes[:2], ["0.4.0", "0.4.0-ci"])
        first = list(registry.writes)
        promote(bundle(), Path("."), registry)
        self.assertEqual(registry.writes, first)

    def test_partial_version_failure_does_not_move_any_alias(self):
        registry = MemoryRegistry()
        registry.fail_tag = "0.4.0-ci"
        with self.assertRaises(RuntimeError):
            promote(bundle(), ".", registry)
        self.assertEqual(registry.writes, ["0.4.0", "0.4.0-ci"])
        registry.fail_tag = None
        promote(bundle(), ".", registry)
        self.assertEqual(registry.writes.count("0.4.0"), 1)
        self.assertIn("latest", registry.tags)

    def test_timeout_after_success_reconciles_without_second_push(self):
        registry = MemoryRegistry()
        registry.uncertain_tag = "0.4.0"
        promote(bundle(), ".", registry)
        self.assertEqual(registry.writes.count("0.4.0"), 1)

    def test_existing_different_immutable_version_rejects_all_writes(self):
        registry = MemoryRegistry()
        registry.tags["0.4.0-ci"] = "sha256:" + "c" * 64
        with self.assertRaisesRegex(ValueError, "immutable"):
            promote(bundle(), ".", registry)
        self.assertEqual(registry.writes, [])

    def test_prerelease_never_advances_stable_alias(self):
        registry = MemoryRegistry()
        promote(bundle("0.4.0-rc.1"), ".", registry)
        self.assertEqual(registry.writes, ["0.4.0-rc.1", "0.4.0-rc.1-ci"])

    def test_older_release_never_moves_newer_stable_or_minor_alias(self):
        registry = MemoryRegistry()
        registry.versions = {"latest": "0.5.0", "ci": "0.5.0", "0.4": "0.4.3", "0.4-ci": "0.4.3"}
        promote(bundle(), ".", registry)
        self.assertEqual(registry.writes, ["0.4.0", "0.4.0-ci"])

    def test_alias_advancing_during_version_push_is_rechecked_before_any_alias_write(self):
        class AdvancedRegistry(MemoryRegistry):
            def copy(self, path, tag):
                result = super().copy(path, tag)
                if tag == "0.4.0-ci":
                    for alias, version in [("latest", "0.5.0"), ("ci", "0.5.0"),
                                           ("0.4", "0.4.3"), ("0.4-ci", "0.4.3")]:
                        self.tags[alias] = "sha256:" + "c" * 64
                        self.versions[alias] = version
                return result
        registry = AdvancedRegistry()
        promote(bundle(), ".", registry)
        self.assertEqual(registry.writes, ["0.4.0", "0.4.0-ci"])
        for alias in ["latest", "ci", "0.4", "0.4-ci"]:
            self.assertEqual(registry.tags[alias], "sha256:" + "c" * 64)

    def test_inspection_auth_proxy_tls_and_rate_errors_never_mean_absent_tag(self):
        registry = Registry("docker.io/opensourceops/agentctl")
        for diagnostic in [b"authentication service returned 404 Not Found",
                           b"proxy returned 404 Not Found", b"x509: certificate signed by unknown authority",
                           b"unauthorized: authentication required", b"toomanyrequests: rate limit exceeded"]:
            with self.subTest(diagnostic=diagnostic), \
                 patch("publish.subprocess.run", return_value=subprocess.CompletedProcess([], 1, b"", diagnostic)):
                with self.assertRaisesRegex(RuntimeError, "inspection failed"):
                    registry.inspect("0.4.0")

    def test_explicit_registry_missing_manifest_is_the_only_absence_signal(self):
        registry = Registry("docker.io/opensourceops/agentctl")
        for diagnostic in [b"reading manifest 0.4.0: manifest unknown: manifest unknown",
                           b"name unknown: repository name not known to registry"]:
            with self.subTest(diagnostic=diagnostic), \
                 patch("publish.subprocess.run", return_value=subprocess.CompletedProcess([], 1, b"", diagnostic)):
                self.assertIsNone(registry.inspect("0.4.0"))

    def test_unavailable_registry_is_not_an_absent_tag(self):
        registry = MemoryRegistry()
        def unavailable(*args, **kwargs):
            raise RuntimeError("unavailable")
        registry.inspect = unavailable
        with self.assertRaises(RuntimeError):
            promote(bundle(), ".", registry)
        self.assertEqual(registry.writes, [])

    def test_disposable_mode_cannot_disable_tls_for_remote_registries(self):
        for destination in ["docker.io/opensourceops/agentctl", "evil.example:5000/agentctl", "127.0.0.1:5000/another"]:
            with self.assertRaises(ValueError):
                Registry(destination, test=True)
        Registry("127.0.0.1:5000/agentctl-test", test=True)
        with self.assertRaises(ValueError):
            Registry("docker.io/another/agentctl")


if __name__ == "__main__":
    unittest.main()
