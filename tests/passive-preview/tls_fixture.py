"""Narrow local TLS-to-HTTP transport around the synthetic preview web service."""

import http.client
import ssl
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from evidence import SCOPE, WEB_HOST

HOP = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
}
MAX_RESPONSE = 2_000_000


class Forward(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if (WEB_HOST, self.path) not in SCOPE or self.headers.get_all("Host") != [WEB_HOST]:
            self.send_error(404)
            return
        lengths = self.headers.get_all("Content-Length") or []
        auth = self.headers.get_all("Authorization") or []
        encoding = self.headers.get_all("Accept-Encoding") or []
        connection_tokens = {
            part.strip().lower()
            for value in self.headers.get_all("Connection") or []
            for part in value.split(",")
        }
        if (
            lengths not in ([], ["0"])
            or self.headers.get_all("Transfer-Encoding")
            or len(auth) > 1
            or len(encoding) > 1
            or connection_tokens & {"host", "authorization", "accept-encoding"}
        ):
            self.send_error(400)
            return
        connection = http.client.HTTPConnection("127.0.0.1", self.server.upstream_port, timeout=15)
        try:
            connection.putrequest("GET", self.path, skip_host=True, skip_accept_encoding=True)
            connection.putheader("Host", WEB_HOST)
            if auth:
                connection.putheader("Authorization", auth[0])
            if encoding:
                connection.putheader("Accept-Encoding", encoding[0])
            connection.endheaders()
            response = connection.getresponse()
            body = response.read(MAX_RESPONSE + 1)
            if len(body) > MAX_RESPONSE:
                self.send_error(502)
                return
            response_headers = response.getheaders()
            excluded = HOP | {
                part.strip().lower()
                for name, value in response_headers
                if name.lower() == "connection"
                for part in value.split(",")
            }
            self.send_response_only(response.status, response.reason)
            for name, value in response_headers:
                if name.lower() not in excluded and name.lower() != "content-length":
                    self.send_header(name, value)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        finally:
            connection.close()

    def log_message(self, *args):
        pass


def start(cert, key, *, port=3443, upstream_port=3000):
    server = ThreadingHTTPServer(("127.0.0.1", port), Forward)
    server.upstream_port = upstream_port
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(cert, key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    return server, worker
