"""Fail closed on incomplete synthetic preview observations."""

import re
from urllib.parse import urlsplit

SCOPE = {
    ("127.0.0.1:3443", "/nl/collection/isolated-room?variant=SET-COVERS"): "SET-COVERS",
    ("127.0.0.1:3443", "/nl/room-planner"): "<html",
    ("127.0.0.1:45671", "/api/published/nl/products/SET-FILLED"): '"sku":"SET-FILLED"',
}
WEB_HOST = "127.0.0.1:3443"
API_HOST = "127.0.0.1:45671"


def scheme_for(host):
    if host == WEB_HOST:
        return "https"
    if host == API_HOST:
        return "http"
    raise ValueError("Unknown scope")


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
    "revision",
    "missing_scope",
    "request_status",
    "history_source",
    "history_id",
}


class EvidenceError(ValueError):
    """A closed diagnostic containing no scanner-controlled text."""

    def __init__(self, bucket, scope=0, status=None):
        if bucket not in FAILURES or scope not in (0, 1, 2, 3):
            raise ValueError("Invalid diagnostic")
        self.bucket = bucket
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
    if key not in SCOPE or uri.scheme != scheme_for(key[0]) or uri.fragment:
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


def history_summary(messages):
    """Return bounded fixed categories only, never raw history."""
    if not isinstance(messages, list):
        return {"shape": "invalid"}
    counts = {
        name: 0
        for name in (
            "scope1",
            "scope2",
            "scope3",
            "other_target",
            "malformed_target",
            "type_proxied",
            "type_temporary",
            "type_zap_user",
            "type_other",
            "status_401",
            "status_200",
            "status_other",
        )
    }
    for item in messages[:100]:
        if not isinstance(item, dict):
            counts["malformed_target"] += 1
            counts["type_other"] += 1
            counts["status_other"] += 1
            continue
        kind = item.get("type")
        kind = kind if isinstance(kind, str) else ""
        counts[
            {"0": "type_temporary", "1": "type_proxied", "15": "type_zap_user"}.get(
                kind, "type_other"
            )
        ] += 1
        try:
            key, scope, status, _ = message_parts(item, set())
            counts[f"scope{scope}"] += 1
            counts[{401: "status_401", 200: "status_200"}.get(status, "status_other")] += 1
        except EvidenceError as error:
            counts["other_target" if error.bucket == "target" else "malformed_target"] += 1
            counts["status_other"] += 1
    return {
        "total": str(len(messages)) if len(messages) < 10 else "10plus",
        **{key: str(value) if value < 10 else "10plus" for key, value in counts.items()},
    }


def proxied_messages(messages):
    """Permit ZAP's temporary site-tree parents, never use them as scan evidence."""
    if not isinstance(messages, list):
        raise EvidenceError("history_type")
    # Core API pagination is capped at 100; an exact cap could hide further records.
    if len(messages) >= 100:
        raise EvidenceError("history_count")
    proxied, ids = [], set()
    connects = 0
    for message in messages:
        if not isinstance(message, dict):
            raise EvidenceError("message_type")
        identity = message.get("id")
        if (
            not isinstance(identity, str)
            or not identity.isascii()
            or not identity.isdigit()
            or identity in ids
        ):
            raise EvidenceError("history_id")
        ids.add(identity)
        kind = message.get("type")
        if kind == "1":  # HistoryReference.TYPE_PROXIED
            proxied.append(message)
            continue
        if kind == "16":  # Optional local CONNECT transport metadata, never evidence.
            connects += 1
            raw = message.get("requestHeader")
            lines = raw.splitlines() if isinstance(raw, str) else []
            if (
                connects > 2
                or not lines
                or lines[0] not in (f"CONNECT {WEB_HOST} HTTP/1.0", f"CONNECT {WEB_HOST} HTTP/1.1")
                or any(line.lower().startswith("authorization:") for line in lines[1:])
            ):
                raise EvidenceError("history_source")
            continue
        if kind != "0":  # Only TYPE_TEMPORARY site-tree parents are permitted.
            raise EvidenceError("history_source")
        raw = message.get("requestHeader")
        if not isinstance(raw, str) or not raw.splitlines():
            raise EvidenceError("history_source")
        parts = raw.splitlines()[0].split()
        if len(parts) != 3 or parts[0] != "GET" or parts[2] != "HTTP/1.1":
            raise EvidenceError("history_source")
        try:
            uri = urlsplit(parts[1])
        except ValueError:
            raise EvidenceError("history_source") from None
        if (
            uri.netloc not in (WEB_HOST, API_HOST)
            or uri.scheme != scheme_for(uri.netloc)
            or uri.query
            or uri.fragment
        ):
            raise EvidenceError("history_source")
        parent = uri.path.rstrip("/")
        if not any(
            uri.netloc == host
            and (
                path.partition("?")[0].startswith(parent + "/")
                or ("?" in path and parent == path.partition("?")[0])
            )
            for host, path in SCOPE
        ):
            raise EvidenceError("history_source")
    return proxied


def validate_messages(messages, expected_auth, revision):
    """Require exact authenticated responses in scanner history, not client-side claims."""
    messages = proxied_messages(messages)
    seen = set()
    for message in messages:
        key, scope, status, auth = message_parts(message, seen)
        if status != 200:
            raise EvidenceError("response_status", scope, status)
        if SCOPE[key] not in message.get("responseBody", ""):
            raise EvidenceError("content", scope, status)
        if auth != [expected_auth[key]]:
            raise EvidenceError("authorization", scope, status)
        if key[0] == WEB_HOST:
            headers = message["responseHeader"].splitlines()[1:]
            revisions = [
                value.strip(" \t")
                for line in headers
                for name, sep, value in [line.partition(":")]
                if sep and name.lower() == "x-piwas-preview-revision"
            ]
            if revisions != [revision]:
                raise EvidenceError("revision", scope, status)
        seen.add(key)
    if seen != set(SCOPE):
        raise EvidenceError("missing_scope")
    return len(seen)


def validate_anonymous(messages):
    messages = proxied_messages(messages)
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
