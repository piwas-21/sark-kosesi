"""Fail closed on incomplete synthetic preview observations."""

from urllib.parse import urlsplit

SCOPE = {
    ("127.0.0.1:3000", "/nl/collection/isolated-room?variant=SET-COVERS"): "SET-COVERS",
    ("127.0.0.1:3000", "/nl/room-planner"): "<html",
    ("127.0.0.1:45671", "/api/published/nl/products/SET-FILLED"): '"sku":"SET-FILLED"',
}


def validate_messages(messages, expected_auth):
    """Require exact authenticated responses in scanner history, not client-side claims."""
    if not isinstance(messages, list):
        raise ValueError("Invalid scanner history")
    seen = set()
    for message in messages:
        request = message.get("requestHeader", "").splitlines()
        response = message.get("responseHeader", "").splitlines()
        if not request or not response:
            raise ValueError("Incomplete scanner message")
        fields = request[0].split()
        if len(fields) != 3 or fields[0] != "GET":
            raise ValueError("Unexpected scanner request")
        uri = urlsplit(fields[1])
        path = uri.path + ("?" + uri.query if uri.query else "")
        key = (uri.netloc, path)
        if uri.scheme != "http" or key not in SCOPE or key in seen:
            raise ValueError("Unexpected or duplicate scanner target")
        if not response[0].startswith("HTTP/1.1 200 "):
            raise ValueError("Protected response was not successful")
        if SCOPE[key] not in message.get("responseBody", ""):
            raise ValueError("Expected fixture content missing from scanner history")
        auth_headers = [line for line in request[1:] if line.lower().startswith("authorization:")]
        if auth_headers != ["Authorization: " + expected_auth[key]]:
            raise ValueError("Scanner history lacks exact authorization")
        seen.add(key)
    if seen != set(SCOPE):
        raise ValueError("Incomplete authenticated scope")
    return len(seen)


def validate_anonymous(messages):
    if not isinstance(messages, list) or len(messages) != len(SCOPE):
        raise ValueError("Missing anonymous observations")
    seen = set()
    for message in messages:
        request = message.get("requestHeader", "").splitlines()
        response = message.get("responseHeader", "").splitlines()
        if not request or not response:
            raise ValueError("Incomplete anonymous message")
        fields = request[0].split()
        if len(fields) != 3 or fields[0] != "GET":
            raise ValueError("Unexpected anonymous request")
        uri = urlsplit(fields[1])
        key = (uri.netloc, uri.path + ("?" + uri.query if uri.query else ""))
        if (
            uri.scheme != "http"
            or key not in SCOPE
            or key in seen
            or not response[0].startswith("HTTP/1.1 401 ")
            or any(line.lower().startswith("authorization:") for line in request[1:])
        ):
            raise ValueError("Anonymous observation is not a protected rejection")
        seen.add(key)
    if seen != set(SCOPE):
        raise ValueError("Incomplete anonymous scope")


def validate_alerts(alerts):
    if not isinstance(alerts, list):
        raise ValueError("Invalid scanner alerts")
    counts = {"Informational": 0, "Low": 0, "Medium": 0, "High": 0}
    for alert in alerts:
        risk = alert.get("risk")
        if risk not in counts:
            raise ValueError("Unexpected scanner severity")
        if not str(alert.get("pluginId", "")).isdigit():
            raise ValueError("Invalid scanner rule identity")
        counts[risk] += 1
    if counts["High"]:
        raise ValueError("High severity passive finding")
    return counts


def validate_scanners(scanners):
    if not isinstance(scanners, list) or not scanners:
        raise ValueError("No passive rules")
    for scanner in scanners:
        if scanner.get("enabled") != "true" or scanner.get("alertThreshold") not in (
            "DEFAULT",
            "LOW",
            "MEDIUM",
        ):
            raise ValueError("Passive rule disabled or threshold weakened")
    return len(scanners)
