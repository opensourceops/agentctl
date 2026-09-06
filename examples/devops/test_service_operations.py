"""Real loopback and local-state regressions; no providers or production services."""
import json
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import tempfile
import threading
import unittest
from unittest.mock import patch

import fixture
from local_service import create_server
import service_operations as service


class ServiceOperationTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="agentctl-service-review-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        root_patch = patch.object(fixture, "ROOT", self.root)
        root_patch.start()
        self.addCleanup(root_patch.stop)

    def write(self, name, value):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        text = value if isinstance(value, str) else json.dumps(value, sort_keys=True) + "\n"
        target.write_bytes(text.encode())
        return text

    def start_server(self, server):
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        thread.start()
        def stop():
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())
        self.addCleanup(stop)
        return server

    def test_snapshot_deploy_real_probe_and_restore_arbitrary_prior_state(self):
        prior = self.write("artifacts/service.json", {"version": "8.4", "healthy": True, "settings": {"feature": "preserve", "replicas": 7}})
        desired = self.write("desired.json", {"version": "9.0", "healthy": False, "settings": {"replicas": 1}})
        self.start_server(create_server(self.root))
        snapshot = service.service_snapshot(fixture, {"desiredPath": "desired.json"})
        self.assertEqual(snapshot["priorText"], prior)
        self.assertEqual((self.root / "artifacts/prior-service.json").read_text(), prior)
        self.assertEqual((self.root / "artifacts/service.json").read_text(), prior)
        fixture.write_atomic("artifacts/service.json", snapshot["desiredText"])
        probe = {"endpointPath": "service-endpoint.json", "expectedText": desired, "requireHealthy": True}
        failed = service.service_probe(fixture, probe)
        self.assertFalse(failed["verified"])
        self.assertEqual(failed["reason"], "observed service is unhealthy")
        fixture.write_atomic("artifacts/service.json", snapshot["priorText"])
        restored = service.service_probe(fixture, {**probe, "expectedText": prior})
        self.assertTrue(restored["verified"])
        self.assertEqual(restored["version"], "8.4")
        self.assertEqual((self.root / "artifacts/service.json").read_bytes(), prior.encode())
        self.assertFalse(service.service_probe(fixture, probe)["verified"])

    def test_probe_rejects_out_of_scope_destinations_before_network(self):
        expected = json.dumps({"version": "1", "healthy": True})
        invalid = ["https://127.0.0.1:8000/service.json", "http://example.com:8000/service.json", "http://localhost:8000/service.json",
                   "http://127.0.0.1/service.json", "http://user@127.0.0.1:8000/service.json", "http://127.0.0.1:8000/other",
                   "http://127.0.0.1:8000/service.json?x=1", "http://127.0.0.1:8000/service.json#fragment", "http://127.0.0.1:99999/service.json"]
        with patch.object(service, "build_opener") as open_network:
            for url in invalid:
                self.write("endpoint.json", {"url": url})
                with self.subTest(url=url), self.assertRaises(ValueError):
                    service.service_probe(fixture, {"endpointPath": "endpoint.json", "expectedText": expected, "requireHealthy": True})
            open_network.assert_not_called()

    def test_probe_does_not_follow_redirects(self):
        hits = []
        class Target(BaseHTTPRequestHandler):
            def do_GET(self):
                hits.append(self.path)
                self.send_response(200)
                self.end_headers()
            def log_message(self, *_):
                pass
        target = self.start_server(ThreadingHTTPServer(("127.0.0.1", 0), Target))
        class Redirect(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(302)
                self.send_header("Location", f"http://127.0.0.1:{target.server_port}/service.json")
                self.end_headers()
            def log_message(self, *_):
                pass
        redirect = self.start_server(ThreadingHTTPServer(("127.0.0.1", 0), Redirect))
        self.write("endpoint.json", {"url": f"http://127.0.0.1:{redirect.server_port}/service.json"})
        with self.assertRaisesRegex(ValueError, "never follow redirects"):
            service.service_probe(fixture, {"endpointPath": "endpoint.json", "expectedText": '{"version":"1","healthy":true}', "requireHealthy": True})
        self.assertEqual(hits, [])

    def test_deploy_is_non_idempotent_and_checks_actual_captured_configuration(self):
        desired = self.write("desired.json", {"version": "2.3", "healthy": True, "userSetting": [1, 2]})
        result = service.service_deploy(fixture, {"desiredPath": "desired.json"})
        self.assertEqual(result["mutation"], 1)
        check = {"expectedText": desired, "requireReady": True}
        self.assertFalse(service.service_check(fixture, check)["verified"])
        self.write("fixtures/retry-ready.txt", "available\n")
        self.assertTrue(service.service_check(fixture, check)["verified"])
        self.assertTrue(service.service_check(fixture, {**check, "requireReady": False})["verified"])
        self.assertEqual(service.service_deploy(fixture, {"desiredPath": "desired.json"})["mutation"], 2)
        self.write("artifacts/service.json", {"version": "unexpected", "healthy": True})
        with self.assertRaisesRegex(ValueError, "differs"):
            service.service_check(fixture, {**check, "requireReady": False})
        self.write("artifacts/service.json", {"version": "2.3", "healthy": False})
        with self.assertRaises(ValueError):
            service.service_check(fixture, {**check, "requireReady": False})

    def test_partial_application_retains_journal_and_rejects_blind_redispatch(self):
        desired = self.write("desired.json", {"version": "new", "healthy": True})
        write = fixture.write_atomic
        def interrupt(name, value):
            if name == "artifacts/mutations.txt":
                raise InterruptedError("simulated death after service write before counter")
            return write(name, value)
        with patch.object(fixture, "write_atomic", side_effect=interrupt), self.assertRaises(InterruptedError):
            service.service_deploy(fixture, {"desiredPath": "desired.json"})
        self.assertEqual((self.root / "artifacts/service.json").read_bytes(), desired.encode())
        self.assertFalse((self.root / "artifacts/mutations.txt").exists())
        journal = json.loads((self.root / "artifacts/deployment-journal.json").read_text())
        self.assertEqual((journal["phase"], journal["nextMutation"]), ("intent", 1))
        with self.assertRaisesRegex(ValueError, "reconciliation"):
            service.service_deploy(fixture, {"desiredPath": "desired.json"})
        self.assertEqual(json.loads((self.root / "artifacts/deployment-journal.json").read_text()), journal)

    def test_matrix_uses_editable_services_thresholds_and_ordered_aggregation(self):
        for name in ["frontend", "batch"]:
            self.write(f"fixtures/{name}.json", {"replicas": 4, "timeoutSeconds": 45})
            self.write(f"fixtures/{name}.py", f'SERVICE = "{name}"\n')
        items = []
        for index, (name, check) in enumerate([(name, check) for name in ["frontend", "batch"] for check in ["syntax", "config"]]):
            output = service.service_check_item(fixture, {"service": name, "check": check, "index": index, "maxReplicas": 5, "maxTimeoutSeconds": 60})
            items.append({"output": output})
        aggregated = service.aggregate_services(fixture, {"items": items})
        self.assertEqual(aggregated["count"], 4)
        self.assertEqual([item["index"] for item in aggregated["checks"]], [0, 1, 2, 3])
        with self.assertRaisesRegex(ValueError, "out of order"):
            service.aggregate_services(fixture, {"items": list(reversed(items))})
        with self.assertRaisesRegex(ValueError, "replicas"):
            service.service_check_item(fixture, {"service": "frontend", "check": "config", "index": 0, "maxReplicas": 3, "maxTimeoutSeconds": 60})
        self.write("fixtures/frontend.py", "def broken(\n")
        with self.assertRaises(SyntaxError):
            service.service_check_item(fixture, {"service": "frontend", "check": "syntax", "index": 0, "maxReplicas": 5, "maxTimeoutSeconds": 60})
        with self.assertRaisesRegex(ValueError, "identifier"):
            service.service_check_item(fixture, {"service": "../outside", "check": "syntax", "index": 0, "maxReplicas": 5, "maxTimeoutSeconds": 60})


if __name__ == "__main__":
    unittest.main()
