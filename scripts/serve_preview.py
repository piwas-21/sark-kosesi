#!/usr/bin/env python3
"""Serve only the non-selling design specimen on loopback."""

import os
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "design" / "prototype"


class PreviewHandler(SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("X-Robots-Tag", "noindex, nofollow")
        self.send_header("Cache-Control", "no-store")
        super().end_headers()


if __name__ == "__main__":
    port = int(os.environ.get("PORT", "48763"))
    server = ThreadingHTTPServer(("127.0.0.1", port), partial(PreviewHandler, directory=str(ROOT)))
    print(f"Design preview: http://localhost:{port}/studio.html", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.server_close()
