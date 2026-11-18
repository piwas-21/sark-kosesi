"""Fail closed on incomplete synthetic preview observations."""

import re
from urllib.parse import urlsplit

SCOPE = {
    ("127.0.0.1:3000", "/nl/collection/isolated-room?variant=SET-COVERS"): "SET-COVERS",
    ("127.0.0.1:3000", "/nl/room-planner"): "<html",
    ("127.0.0.1:45671", "/api/published/nl/products/SET-FILLED"): '"sku":"SET-FILLED"',
}
SCOPE_INDEX = {key: index for index, key in enumerate(SCOPE, 1)}
STATUS = re.compile(r"HTTP/1\.1 ([1-5][0-9]{2})(?: [\x20-\x7e\x80-\xff]*)?\Z")
SAFE_STATUSES = {200, 401, 403, 404, 500, 502, 503}
FAILURES = {
    "history_type",
    "history_count",
    "message_type",
    "missing_headers",
    "request_line",
    "target",
    "duplicate",
    "response_line",
    "response_status",
    "content",
    "authorization",
    "missing_scope",
    "request_status",
}


class EvidenceError(ValueError):
    """A closed diagnostic containing no scanner-controlled text."""

    def __init__(self, bucket, scope=0, status=None):
        if bucket not in FAILURES or scope not in (0, 1, 2, 3):
            raise ValueError("Invalid diagnostic")
        safe_status = str(status) if type(status) is int and status in SAFE_STATUSES else "other"
        super().__init__(f"bucket={bucket} scope={scope} status={safe_status}")


def status_code(line, scope):
    match = STATUS.fullmatch(line)
    if match is None:
        raise EvidenceError("response_line", scope)
    return int(match.group(1))


def message_parts(message, seen):
    if not isinstance(message, dict):
        raise EvidenceError("message_type")
    request_raw = message.get("requestHeader")
    response_raw = message.get("responseHeader")
    if not isinstance(request_raw, str) or not isinstance(response_raw, str):
        raise EvidenceError("missing_headers")
    request, response = request_raw.splitlines(), response_raw.splitlines()
    if not request or not response:
        raise EvidenceError("missing_headers")
    fields = request[0].split()
    if len(fields) != 3 or fields[0] != "GET" or fields[2] != "HTTP/1.1":
        raise EvidenceError("request_line")
    try:
        uri = urlsplit(fields[1])
    except ValueError:
        raise EvidenceError("target") from None
    path = uri.path + ("?" + uri.query if uri.query else "")
    key = (uri.netloc, path)
    if uri.scheme != "http" or key not in SCOPE or uri.fragment:
        raise EvidenceError("target")
    scope = SCOPE_INDEX[key]
    if key in seen:
        raise EvidenceError("duplicate", scope)
    auth = []
    for line in request[1:]:
        name, sep, value = line.partition(":")
        if sep and name.lower() == "authorization":
            auth.append(value.strip(" \t"))
    return key, scope, status_code(response[0], scope), auth


def validate_messages(messages, expected_auth):
    """Require exact authenticated responses in scanner history, not client-side claims."""
    if not isinstance(messages, list):
        raise EvidenceError("history_type")
    seen = set()
    for message in messages:
        key, scope, status, auth = message_parts(message, seen)
        if status != 200:
            raise EvidenceError("response_status", scope, status)
        if SCOPE[key] not in message.get("responseBody", ""):
            raise EvidenceError("content", scope, status)
        if auth != [expected_auth[key]]:
            raise EvidenceError("authorization", scope, status)
        seen.add(key)
    if seen != set(SCOPE):
        raise EvidenceError("missing_scope")
    return len(seen)


def validate_anonymous(messages):
    if not isinstance(messages, list):
        raise EvidenceError("history_type")
    if len(messages) != len(SCOPE):
        raise EvidenceError("history_count")
    seen = set()
    for message in messages:
        key, scope, status, auth = message_parts(message, seen)
        if status != 401:
            raise EvidenceError("response_status", scope, status)
        if auth:
            raise EvidenceError("authorization", scope, status)
        seen.add(key)
    if seen != set(SCOPE):
        raise EvidenceError("missing_scope")


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
