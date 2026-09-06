"""Read a bounded HTTP response preview; no arbitrary command execution."""
import argparse
import json
from urllib.parse import urlsplit
import urllib3


def preview(url, maximum=1024):
    parsed = urlsplit(url)
    if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username or parsed.password:
        raise ValueError("provide an HTTP(S) URL without embedded credentials")
    if type(maximum) is not int or not 1 <= maximum <= 4096:
        raise ValueError("preview limit must be between 1 and 4096 bytes")
    with urllib3.PoolManager(timeout=urllib3.Timeout(connect=3, read=3)) as client:
        response = client.request("GET", url, preload_content=False, redirect=False, retries=False)
        try:
            content = response.read(maximum, decode_content=False)
            return {"status": response.status, "bytes": len(content), "preview": content.decode("utf-8", errors="replace")}
        finally:
            response.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url")
    parser.add_argument("--maximum", type=int, default=1024)
    options = parser.parse_args()
    print(json.dumps(preview(options.url, options.maximum)))
