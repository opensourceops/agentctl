import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from app import preview


class PreviewTests(unittest.TestCase):
    def test_bounded_preview_uses_real_http_and_preserves_status(self):
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"deployment healthy\n" * 8)
            def log_message(self, *_):
                pass
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        worker = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True)
        worker.start()
        try:
            self.assertEqual(preview(f"http://127.0.0.1:{server.server_port}/", 10),
                             {"status": 200, "bytes": 10, "preview": "deployment"})
        finally:
            server.shutdown()
            server.server_close()
            worker.join(timeout=2)
            self.assertFalse(worker.is_alive())

    def test_invalid_destination_and_bounds_do_not_send(self):
        for url in ["file:///etc/passwd", "http://user:password@example.com", "not-a-url"]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                preview(url)
        for maximum in [0, 4097, True]:
            with self.subTest(maximum=maximum), self.assertRaises(ValueError):
                preview("http://127.0.0.1:1", maximum)


if __name__ == "__main__":
    unittest.main()
