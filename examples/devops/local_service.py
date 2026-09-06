#!/usr/bin/env python3
"""Serve the disposable example state on loopback; stop with Ctrl+C. No production service."""
import argparse
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path


def create_server(root, port=0):
    directory = Path(root).resolve() / "artifacts"
    if not (directory / "service.json").is_file():
        raise ValueError("Run setup.py first to prepare the disposable service state")
    server = ThreadingHTTPServer(("127.0.0.1", port), partial(SimpleHTTPRequestHandler, directory=str(directory)))
    (Path(root) / "service-endpoint.json").write_text(json.dumps({"url": f"http://127.0.0.1:{server.server_port}/service.json"}) + "\n")
    return server


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()
    server = create_server(Path(__file__).resolve().parent, args.port)
    print(f"Disposable service: http://127.0.0.1:{server.server_port}/service.json", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
