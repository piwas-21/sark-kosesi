"""Evaluate redacted quote observations; caller must establish independent custody.

This pure evaluator does not launch a load, read credentials, authenticate an API,
verify a database or establish release evidence. Intervals are client-side samples.
"""

import math
from collections import Counter

MAX_REQUESTS = 3000
MAX_REQUEST_INTERVAL_MS = 3500
MIN_STEADY_IDLE_MS = 10_100
MAX_STEADY_IDLE_MS = 11_500
MIN_STEADY_REQUESTS = 40
MAX_STEADY_CYCLE_MS = MAX_REQUEST_INTERVAL_MS + MAX_STEADY_IDLE_MS
LEDGERS = {
    "stock_holds",
    "stock_hold_lines",
    "basket_hold_groups",
    "stock_movements",
    "checkout_orders",
    "payment_attempts",
    "commerce_outbox",
}
STOCK = [["SEAT-COVER", 100, 0, 0], ["SEAT-FOAM", 100, 0, 0]]
TOTAL = {"gross_minor": 38245, "net_minor": 31607, "vat_minor": 6638}
QUOTE = {
    "sku": "SET-FILLED",
    "quantity": 3,
    "locale": "nl",
    "currency": "EUR",
    "indicative": True,
    "total": TOTAL,
}


def invalid():
    raise ValueError("invalid quote observations")


def integer(value, minimum, maximum):
    return type(value) is int and minimum <= value <= maximum


def p95(values):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[math.ceil(len(ordered) * 0.95) - 1]


def evaluate(document):
    """Return bounded measurements, never credentials or release-gate approval."""
    if not isinstance(document, dict) or set(document) != {
        "schemaVersion",
        "steady_start_ms",
        "steady_end_ms",
        "burst_end_ms",
        "requests",
        "stock_before",
        "stock_after",
        "ledgers_before",
        "ledgers_after",
        "quote_count_before",
        "quote_count_after",
    }:
        invalid()
    if document["schemaVersion"] != 1 or type(document["schemaVersion"]) is not int:
        invalid()
    start, end, burst_end = (
        document["steady_start_ms"],
        document["steady_end_ms"],
        document["burst_end_ms"],
    )
    if not all(integer(value, 0, 9_007_199_254_740_991) for value in (start, end, burst_end)):
        invalid()
    if not (600_000 <= end - start <= 630_000 and end <= burst_end <= end + 60_000):
        invalid()
    for name in ("stock_before", "stock_after"):
        value = document[name]
        if value != STOCK or any(type(amount) is not int for row in value for amount in row[1:]):
            invalid()
    empty_ledgers = dict.fromkeys(LEDGERS, 0)
    for name in ("ledgers_before", "ledgers_after"):
        value = document[name]
        if value != empty_ledgers or any(type(amount) is not int for amount in value.values()):
            invalid()
    if not integer(document["quote_count_before"], 0, 0) or not integer(
        document["quote_count_after"], 0, MAX_REQUESTS
    ):
        invalid()
    requests = document["requests"]
    if not isinstance(requests, list) or not 75 <= len(requests) <= MAX_REQUESTS:
        invalid()
    groups = {"steady": [], "burst": []}
    quota = Counter()
    identities = set()
    for row in requests:
        if not isinstance(row, dict) or set(row) != {
            "workload",
            "owner",
            "sequence",
            "started_ms",
            "finished_ms",
            "status",
            "replayed",
            "quote",
        }:
            invalid()
        workload = row["workload"]
        if type(workload) is not str or workload not in groups:
            invalid()
        owner, sequence, first, last, status = (
            row["owner"],
            row["sequence"],
            row["started_ms"],
            row["finished_ms"],
            row["status"],
        )
        if not (
            integer(owner, 0, 24 if workload == "steady" else 49)
            and integer(sequence, 0, 9999)
            and integer(first, start, burst_end)
            and integer(last, first, burst_end)
            and last - first <= MAX_REQUEST_INTERVAL_MS
            and integer(status, 0, 599)
            and (status == 0 or status >= 100)
        ):
            invalid()
        if workload == "steady" and first >= end:
            invalid()
        if workload == "burst" and (first < start + 600_000 or sequence != 0):
            invalid()
        if row["replayed"] not in (True, False, None) or type(row["replayed"]) not in (
            bool,
            type(None),
        ):
            invalid()
        if row["quote"] is not None:
            quote = row["quote"]
            if (
                quote != QUOTE
                or type(quote["quantity"]) is not int
                or type(quote["indicative"]) is not bool
                or any(type(amount) is not int for amount in quote["total"].values())
            ):
                invalid()
        success = status == 200 and row["replayed"] is False and row["quote"] == QUOTE
        if status == 200 and not success:
            invalid()
        identity = (workload, owner, sequence)
        if identity in identities:
            invalid()
        identities.add(identity)
        quota[(workload, owner, first // 60_000)] += 1
        groups[workload].append((row, success))
    if max(quota.values()) > 10:
        invalid()
    for owner in range(25):
        owned = sorted(
            (row for row, _ in groups["steady"] if row["owner"] == owner),
            key=lambda row: row["sequence"],
        )
        if (
            len(owned) < MIN_STEADY_REQUESTS
            or [row["sequence"] for row in owned] != list(range(len(owned)))
            or owned[0]["started_ms"] > start + 10_000
            or owned[-1]["started_ms"] < end - MAX_STEADY_CYCLE_MS
            or any(
                not MIN_STEADY_IDLE_MS
                <= later["started_ms"] - earlier["finished_ms"]
                <= MAX_STEADY_IDLE_MS
                for earlier, later in zip(owned, owned[1:], strict=False)
            )
        ):
            invalid()
    burst = groups["burst"]
    if len(burst) != 50 or {row["owner"] for row, _ in burst} != set(range(50)):
        invalid()
    success_count = sum(success for group in groups.values() for _, success in group)
    if document["quote_count_after"] != success_count:
        invalid()
    metrics = {}
    for workload, group in groups.items():
        durations = [row["finished_ms"] - row["started_ms"] for row, ok in group if ok]
        failures = sum(not ok for _, ok in group)
        metrics[workload] = {
            "requests": len(group),
            "failures": failures,
            "p95_elapsed_ms": p95(durations),
            "within_budget": bool(durations)
            and p95(durations) <= 500
            and failures * 100 <= len(group),
        }
    # A zero-millisecond interval cannot establish overlap at this resolution.
    nonzero = [row for row, _ in burst if row["finished_ms"] > row["started_ms"]]
    points = sorted(
        [(row["started_ms"], 1) for row in nonzero] + [(row["finished_ms"], -1) for row in nonzero]
    )
    active = maximum = 0
    for _, delta in points:
        active += delta
        maximum = max(maximum, active)
    metrics["burst"]["max_client_inflight"] = maximum
    metrics["burst"]["within_budget"] &= maximum >= 50
    return metrics
