"""Exercise the pinned scanner against disposable local Basic/Bearer 401 stand-ins."""

import base64
import contextlib
import io
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import run_scan
from evidence import SCOPE, WEB_HOST, EvidenceError


class Fixture(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        host = (
            WEB_HOST if self.server.server_port == 3000 else f"127.0.0.1:{self.server.server_port}"
        )
        marker = SCOPE.get((host, self.path))
        token = os.environ["PREVIEW_TOKEN"]
        expected = (
            f"Bearer {token}"
            if self.server.server_port == 45671
            else "Basic " + base64.b64encode(f"preview:{token}".encode()).decode()
        )
        authorized = marker is not None and self.headers.get("Authorization") == expected
        body = (marker if authorized else "denied").encode()
        self.send_response(200 if authorized else 401)
        self.send_header(
            "Content-Type", "application/json" if self.server.server_port == 45671 else "text/html"
        )
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("X-Piwas-Preview-Revision", os.environ["ANALYSIS_REVISION"])
        self.send_header("Cache-Control", "private, no-store")
        self.send_header("X-Robots-Tag", "noindex, nofollow")
        if authorized and self.server.server_port == 3000:
            self.send_header("Content-Security-Policy", "default-src 'self'")
        self.send_header("Content-Length", str(len(body)))
        if not authorized and self.server.server_port != 45671:
            self.send_header("WWW-Authenticate", 'Basic realm="preview"')
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main():
    servers = [ThreadingHTTPServer(("127.0.0.1", port), Fixture) for port in (3000, 45671)]
    workers = [threading.Thread(target=server.serve_forever, daemon=True) for server in servers]
    for worker in workers:
        worker.start()
    try:
        # Forward only the driver's pre-sanitized diagnostics, not success evidence.
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            try:
                run_scan.main()
            except EvidenceError as exc:
                print(f"Preflight failed at {run_scan.PHASE} ({exc})")
                raise
            except Exception as exc:
                print(f"Preflight failed at {run_scan.PHASE} ({type(exc).__name__})")
                raise
        print("Pinned passive scanner preflight passed")
    except Exception:
        print(output.getvalue(), end="")
        raise SystemExit(1) from None
    finally:
        for server in servers:
            server.shutdown()
            server.server_close()
        for worker in workers:
            worker.join()


if __name__ == "__main__":
    main()
