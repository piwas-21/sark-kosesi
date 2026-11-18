"""Drive only literal local GETs through a private ZAP passive proxy."""

import base64
import http.client
import json
import os
import ssl
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlsplit

import tls_fixture
from evidence import (
    SCOPE,
    WEB_HOST,
    EvidenceError,
    history_summary,
    scheme_for,
    validate_alerts,
    validate_anonymous,
    validate_messages,
    validate_scanners,
)

TOKEN = os.environ["PREVIEW_TOKEN"]
PHASE = "startup"
TLS_CONTEXT = None


def zap(path):
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=10)
    try:
        connection.request("GET", path)
        response = connection.getresponse()
        if response.status != 200:
            raise ValueError("Scanner API failed")
        return json.loads(response.read(2_000_000))
    finally:
        connection.close()


def request(target, authorization=None, context=None):
    uri = urlsplit(target)
    headers = {"Accept-Encoding": "identity"}
    if authorization:
        headers["Authorization"] = authorization
    if uri.scheme == "https" and uri.netloc == WEB_HOST:
        headers["Host"] = WEB_HOST
        connection = http.client.HTTPSConnection(
            "127.0.0.1", 8080, timeout=15, context=context or TLS_CONTEXT
        )
        connection.set_tunnel("127.0.0.1", 3443, headers={"Host": WEB_HOST})
        path = uri.path + ("?" + uri.query if uri.query else "")
    elif uri.scheme == "http" and uri.netloc.startswith("127.0.0.1:"):
        connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=15)
        path = target
    else:
        raise ValueError("Unexpected target")
    try:
        connection.request("GET", path, headers=headers)
        response = connection.getresponse()
        status = response.status
        response.read(2_000_000)
        return status
    finally:
        connection.close()


def scanner_ca():
    connection = http.client.HTTPConnection("127.0.0.1", 8080, timeout=10)
    try:
        connection.request("GET", "/OTHER/network/other/rootCaCert/")
        response = connection.getresponse()
        if response.status != 200:
            raise ValueError("Scanner CA unavailable")
        pem = response.read(65536).decode("ascii")
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.load_verify_locations(cadata=pem)
        return context
    finally:
        connection.close()


def verify_fixture_tls():
    """Check direct fixture certificate and authenticated header forwarding."""
    cert = "/tmp/fixture-cert.pem"
    verified = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    verified.load_verify_locations(cafile=cert)
    rejected = ssl.create_default_context()
    path = "/nl/room-planner"
    basic = base64.b64encode(f"preview:{TOKEN}".encode()).decode()

    def fetch(connection, auth=None):
        try:
            headers = {"Host": WEB_HOST, "Accept-Encoding": "identity"}
            if auth:
                headers["Authorization"] = "Basic " + basic
            connection.request("GET", path, headers=headers)
            response = connection.getresponse()
            return response.status, dict(response.getheaders()), response.read(2_000_000)
        finally:
            connection.close()

    try:
        fetch(http.client.HTTPSConnection("127.0.0.1", 3443, context=rejected, timeout=5))
    except ssl.SSLCertVerificationError:
        pass
    else:
        raise ValueError("Untrusted fixture certificate accepted")
    for auth in (False, True):
        direct = fetch(http.client.HTTPConnection("127.0.0.1", 3000, timeout=10), auth)
        secure = fetch(
            http.client.HTTPSConnection("127.0.0.1", 3443, context=verified, timeout=10), auth
        )
        if direct[0] != (200 if auth else 401) or secure[0] != direct[0]:
            raise ValueError("Fixture forwarding status differs")
        for key in ("cache-control", "x-robots-tag", "x-content-type-options", "www-authenticate"):
            a = {k.lower(): v for k, v in direct[1].items()}
            b = {k.lower(): v for k, v in secure[1].items()}
            if a.get(key) != b.get(key):
                raise ValueError("Fixture forwarding header differs")
        if b.get("x-piwas-preview-revision") != os.environ["ANALYSIS_REVISION"]:
            raise ValueError("Fixture revision header missing")
        if auth and not b.get("content-security-policy", "").startswith("default-src"):
            raise ValueError("Fixture security policy missing")
        if auth and b"<html" not in secure[2]:
            raise ValueError("Fixture forwarding content missing")
        if not auth and direct[2] != secure[2]:
            raise ValueError("Fixture forwarding content differs")


def drained():
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        value = zap("/JSON/pscan/view/recordsToScan/").get("recordsToScan")
        tasks = zap("/JSON/pscan/view/currentTasks/").get("currentTasks")
        if not isinstance(tasks, list):
            raise ValueError("Invalid passive task status")
        if value == "0" and not tasks:
            return
        if not isinstance(value, str) or not value.isdigit():
            raise ValueError("Invalid passive queue status")
        time.sleep(0.25)
    raise ValueError("Passive scanner did not finish")


def history():
    return zap("/JSON/core/view/messages/?start=0&count=100").get("messages")


def alerts():
    result = zap("/JSON/core/view/alerts/?start=0&count=1000").get("alerts")
    if not isinstance(result, list) or len(result) >= 1000:
        raise ValueError("Scanner alert list missing or truncated")
    return result


def reject_history(messages, expected_auth):
    try:
        validate_messages(messages, expected_auth, os.environ["ANALYSIS_REVISION"])
    except ValueError:
        return
    raise ValueError("Negative history control unexpectedly passed")


def reset():
    if zap("/JSON/core/view/mode/").get("mode") != "safe":
        raise ValueError("Safe mode not confirmed")
    if zap("/JSON/core/action/newSession/?name=&overwrite=true").get("Result") != "OK":
        raise ValueError("Scanner session reset failed")
    if history() != [] or alerts() != []:
        raise ValueError("Scanner session is not empty")


class KnownFinding(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.end_headers()
        self.wfile.write(b"<html><body>synthetic scanner control</body></html>")

    def log_message(self, *args):
        pass


def assessment():
    global PHASE, TLS_CONTEXT
    deadline = time.monotonic() + 90
    while True:
        try:
            version = zap("/JSON/core/view/version/").get("version")
            if (
                not isinstance(version, str)
                or not version
                or any(c not in "0123456789." for c in version)
            ):
                raise ValueError("Invalid scanner version")
            break
        except (OSError, ValueError):
            if time.monotonic() >= deadline:
                raise ValueError("Scanner did not start") from None
            time.sleep(0.5)
    TLS_CONTEXT = scanner_ca()
    try:
        request(
            "https://" + WEB_HOST + "/nl/room-planner",
            context=ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT),
        )
    except ssl.SSLCertVerificationError:
        pass
    else:
        raise ValueError("Untrusted scanner certificate accepted")
    if zap("/JSON/core/action/setMode/?mode=safe").get("Result") != "OK":
        raise ValueError("Cannot select safe mode")
    if zap("/JSON/core/view/mode/").get("mode") != "safe":
        raise ValueError("Safe mode not confirmed")
    if zap("/JSON/pscan/action/setEnabled/?enabled=true").get("Result") != "OK":
        raise ValueError("Cannot enable passive scanning")
    if zap("/JSON/pscan/action/enableAllScanners/").get("Result") != "OK":
        raise ValueError("Cannot enable passive rules")
    if zap("/JSON/pscan/view/maxAlertsPerRule/").get("maxAlertsPerRule") != "0":
        raise ValueError("Passive alert cap is enabled")
    if zap("/JSON/pscan/view/maxBodySizeInBytes/").get("maxBodySizeInBytes") != "0":
        raise ValueError("Passive body limit is enabled")
    scanner_count = validate_scanners(zap("/JSON/pscan/view/scanners/").get("scanners"))
    reset()
    PHASE = "known-finding-control"
    server = HTTPServer(("127.0.0.1", 0), KnownFinding)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        if request(f"http://127.0.0.1:{server.server_port}/control") != 200:
            raise ValueError("Known finding control failed")
        drained()
        found = alerts()
        if not isinstance(found, list) or not any(a.get("pluginId") == "10021" for a in found):
            raise ValueError("Known passive rule did not detect the control")
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
    reset()
    PHASE = "anonymous-control"
    for index, (host, path) in enumerate(SCOPE, 1):
        status = request(f"{scheme_for(host)}://{host}{path}")
        if status != 401:
            raise EvidenceError("request_status", index, status)
    drained()
    anonymous_history = history()
    try:
        validate_anonymous(anonymous_history)
    except EvidenceError:
        print(
            "Anonymous history categories: "
            + json.dumps(history_summary(anonymous_history), sort_keys=True)
        )
        raise
    basic = base64.b64encode(f"preview:{TOKEN}".encode()).decode()
    expected_auth = {
        (host, path): (f"Bearer {TOKEN}" if host.endswith(":45671") else f"Basic {basic}")
        for host, path in SCOPE
    }
    reject_history(history(), expected_auth)
    reset()
    reject_history(history(), expected_auth)
    PHASE = "authenticated-assessment"
    for index, (host, path) in enumerate(SCOPE, 1):
        auth = expected_auth[(host, path)]
        status = request(f"{scheme_for(host)}://{host}{path}", auth)
        if status != 200:
            raise EvidenceError("request_status", index, status)
    drained()
    authenticated_history = history()
    try:
        observed = validate_messages(
            authenticated_history, expected_auth, os.environ["ANALYSIS_REVISION"]
        )
    except EvidenceError:
        print(
            "Authenticated history categories: "
            + json.dumps(history_summary(authenticated_history), sort_keys=True)
        )
        raise
    findings = alerts()
    try:
        counts = validate_alerts(findings)
    except ValueError:
        high_rules = sorted(
            {
                str(a.get("pluginId"))
                for a in findings
                if a.get("risk") == "High" and str(a.get("pluginId", "")).isdigit()
            }
        )
        if high_rules:
            print("High severity rule IDs: " + ", ".join(high_rules))
        raise
    rules = sorted({str(a["pluginId"]) for a in findings})
    print(
        json.dumps(
            {
                "authenticated_responses": observed,
                "enabled_passive_rules": scanner_count,
                "alerts": counts,
                "rule_ids": rules,
                "known_rule": "10021",
                "scanner_version": version,
                "scanner_digest": "sha256:c2d63bbb2d907a5c5257914fb20e26e969c92b25f265def7f89f08203f912882",
                "scanner_platform": "linux/amd64",
                "negative_controls": ["empty", "anonymous"],
                "scope": "isolated-staging-config-loopback-tls-web-http-api",
                "tls_client_trust": "instance-scanner-ca-only",
                "tls_upstream_trust": "scanner-upstream-not-verified;fixture-verified-separately",
                "revision": os.environ["ANALYSIS_REVISION"],
            },
            sort_keys=True,
        )
    )


def main():
    server, worker = tls_fixture.start("/tmp/fixture-cert.pem", "/tmp/fixture-key.pem")
    try:
        verify_fixture_tls()
        assessment()
    finally:
        server.shutdown()
        server.server_close()
        worker.join()


if __name__ == "__main__":
    try:
        main()
    except EvidenceError as exc:
        print(f"Passive preview assessment failed at {PHASE} ({exc})")
        raise SystemExit(1) from None
    except Exception as exc:
        # No message, URL, header, body, path, credential or raw scanner data is printed.
        print(f"Passive preview assessment failed at {PHASE} ({type(exc).__name__})")
        raise SystemExit(1) from None
