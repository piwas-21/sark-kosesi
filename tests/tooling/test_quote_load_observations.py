import copy
import unittest

import quote_load_observations as observations


def fixture():
    start = 1_800_000_000_000
    requests = []
    for owner in range(25):
        for sequence in range(59):
            first = start + sequence * 10_200
            requests.append(
                {
                    "workload": "steady",
                    "owner": owner,
                    "sequence": sequence,
                    "started_ms": first,
                    "finished_ms": first + 100,
                    "status": 200,
                    "replayed": False,
                    "quote": copy.deepcopy(observations.QUOTE),
                }
            )
    for owner in range(50):
        requests.append(
            {
                "workload": "burst",
                "owner": owner,
                "sequence": 0,
                "started_ms": start + 600_000,
                "finished_ms": start + 600_100,
                "status": 200,
                "replayed": False,
                "quote": copy.deepcopy(observations.QUOTE),
            }
        )
    return {
        "schemaVersion": 1,
        "steady_start_ms": start,
        "steady_end_ms": start + 600_000,
        "burst_end_ms": start + 600_100,
        "requests": requests,
        "stock_before": copy.deepcopy(observations.STOCK),
        "stock_after": copy.deepcopy(observations.STOCK),
        "ledgers_before": dict.fromkeys(observations.LEDGERS, 0),
        "ledgers_after": dict.fromkeys(observations.LEDGERS, 0),
        "quote_count_before": 0,
        "quote_count_after": len(requests),
    }


class QuoteLoadObservations(unittest.TestCase):
    def test_literal_25_by_10_minutes_and_50_overlapping_requests(self):
        result = observations.evaluate(fixture())
        self.assertEqual(result["steady"]["requests"], 1475)
        self.assertEqual(result["steady"]["p95_elapsed_ms"], 100)
        self.assertEqual(result["burst"]["max_client_inflight"], 50)
        self.assertTrue(result["steady"]["within_budget"])
        self.assertTrue(result["burst"]["within_budget"])

    def test_independent_effect_and_workload_corruption_fails_closed(self):
        for mutate in (
            lambda d: d["stock_after"][0].__setitem__(1, 99),
            lambda d: d["ledgers_after"].__setitem__("commerce_outbox", 1),
            lambda d: d.__setitem__("quote_count_after", d["quote_count_after"] - 1),
            lambda d: d["requests"][0].__setitem__("replayed", True),
            lambda d: d["requests"][0].__setitem__("quote", {"private": "secret-marker"}),
            lambda d: d["requests"][-1].__setitem__("owner", 48),
            lambda d: d["requests"][0].__setitem__("unknown", "secret-marker"),
            lambda d: d.__setitem__("steady_end_ms", d["steady_start_ms"] + 599_999),
            lambda d: d["requests"][1].__setitem__("started_ms", d["requests"][0]["finished_ms"]),
        ):
            sample = fixture()
            mutate(sample)
            with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
                observations.evaluate(sample)

    def test_latency_error_rate_and_overlap_do_not_turn_failures_green(self):
        slow = fixture()
        for row in slow["requests"][-50:]:
            row["finished_ms"] = row["started_ms"] + 501
        slow["burst_end_ms"] += 401
        self.assertFalse(observations.evaluate(slow)["burst"]["within_budget"])
        serial = fixture()
        for index, row in enumerate(serial["requests"][-50:]):
            row["started_ms"] += index * 100
            row["finished_ms"] += index * 100
        serial["burst_end_ms"] += 4900
        result = observations.evaluate(serial)["burst"]
        self.assertEqual(result["max_client_inflight"], 1)
        self.assertFalse(result["within_budget"])
        zero = fixture()
        zero["requests"][-1]["finished_ms"] = zero["requests"][-1]["started_ms"]
        self.assertEqual(observations.evaluate(zero)["burst"]["max_client_inflight"], 49)
        errors = fixture()
        row = errors["requests"][-1]
        row.update(status=503, replayed=None, quote=None)
        errors["quote_count_after"] -= 1
        self.assertFalse(observations.evaluate(errors)["burst"]["within_budget"])
        errors["quote_count_after"] += 1
        with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
            observations.evaluate(errors)

        steady_error = fixture()
        steady_error["requests"][0].update(status=429, replayed=None, quote=None)
        steady_error["quote_count_after"] -= 1
        self.assertTrue(observations.evaluate(steady_error)["steady"]["within_budget"])

        deadline = fixture()
        row = deadline["requests"][-1]
        row.update(status=0, replayed=None, quote=None)
        row["finished_ms"] = row["started_ms"] + 3500
        deadline["burst_end_ms"] = row["finished_ms"]
        deadline["quote_count_after"] -= 1
        self.assertEqual(observations.evaluate(deadline)["burst"]["failures"], 1)

    def test_burst_observations_must_start_after_the_steady_window(self):
        early = fixture()
        early["requests"][-1]["started_ms"] = early["steady_end_ms"] - 1
        with self.assertRaisesRegex(ValueError, "^quote burst started before"):
            observations.evaluate(early)

    def test_last_valid_cycle_can_end_just_outside_nominal_ten_second_edge(self):
        full = fixture()
        for row in full["requests"][:-50]:
            started = full["steady_start_ms"] + 10_170 * row["sequence"]
            row["started_ms"] = started
            row["finished_ms"] = started + 20
        self.assertTrue(observations.evaluate(full)["steady"]["within_budget"])
        truncated = copy.deepcopy(full)
        truncated["requests"].pop(58)
        truncated["quote_count_after"] -= 1
        with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
            observations.evaluate(truncated)

    def test_sparse_owner_and_missing_middle_attempt_are_rejected(self):
        sparse = fixture()
        sparse["requests"] = [
            row
            for row in sparse["requests"]
            if row["workload"] != "steady" or row["owner"] != 0 or row["sequence"] in (0, 58)
        ]
        for row in sparse["requests"]:
            if row["workload"] == "steady" and row["owner"] == 0 and row["sequence"] == 58:
                row["sequence"] = 1
        sparse["quote_count_after"] = len(sparse["requests"])
        with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
            observations.evaluate(sparse)

        gap = fixture()
        gap["requests"] = [
            row
            for row in gap["requests"]
            if (row["workload"], row["owner"], row["sequence"]) != ("steady", 0, 20)
        ]
        for row in gap["requests"]:
            if row["workload"] == "steady" and row["owner"] == 0 and row["sequence"] > 20:
                row["sequence"] -= 1
        gap["quote_count_after"] = len(gap["requests"])
        with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
            observations.evaluate(gap)

    def test_forty_attempts_fit_the_worst_permitted_cycle(self):
        sample = fixture()
        base = sample["steady_start_ms"]
        sample["requests"] = [
            row for row in sample["requests"] if row["workload"] == "burst" or row["sequence"] < 40
        ]
        for row in sample["requests"]:
            if row["workload"] == "steady":
                row["started_ms"] = base + 5000 + row["sequence"] * 15_000
                row["finished_ms"] = row["started_ms"] + 3500
        sample["quote_count_after"] = len(sample["requests"])
        result = observations.evaluate(sample)
        self.assertEqual(result["steady"]["requests"], 1000)
        self.assertFalse(result["steady"]["within_budget"])

    def test_rejects_booleans_or_extra_fields_as_numeric_proof(self):
        for mutate in (
            lambda d: d["stock_before"][0].__setitem__(2, False),
            lambda d: d["ledgers_before"].__setitem__("stock_holds", False),
            lambda d: d["requests"][0].__setitem__("owner", False),
            lambda d: d.__setitem__("private", "secret-marker"),
        ):
            sample = fixture()
            mutate(sample)
            with self.assertRaisesRegex(ValueError, "^invalid quote observations$"):
                observations.evaluate(sample)
