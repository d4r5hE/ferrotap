"""HTTP/2 client through a proxy (stand-in for `curl --http2`, which this PC's curl builds lack).
Usage: python h2_client.py PROXY_PORT CA_PEM [URL]"""
import ssl
import sys

import httpx

port, ca = sys.argv[1], sys.argv[2]
url = sys.argv[3] if len(sys.argv) > 3 else "https://nghttp2.org/httpbin/get"
ctx = ssl.create_default_context(cafile=ca)
with httpx.Client(http2=True, verify=ctx, proxy=f"http://127.0.0.1:{port}", timeout=30) as c:
    r = c.get(url)
    print(f"port={port} code={r.status_code} ver={r.http_version} bytes={len(r.content)}")
