"""Offline controls for scanner evidence, not a substitute for the hosted scan."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "passive-preview"))
from evidence import (
    SCOPE,
    EvidenceError,
    history_summary,
    validate_alerts,
    validate_anonymous,
    validate_messages,
    validate_scanners,
)  # noqa: E402


def message(host, path, body, status=200):
    return {
        "requestHeader": f"GET http://{host}{path} HTTP/1.1\r\nAuthorization: synthetic\r\n",
        "responseHeader": f"HTTP/1.1 {status} OK\r\n",
        "responseBody": body,
    }


class PassiveEvidenceTest(unittest.TestCase):
    def test_complete_history_and_negative_controls(self):
        good = [message(host, path, marker) for (host, path), marker in SCOPE.items()]
        expected = {key: "synthetic" for key in SCOPE}
        self.assertEqual(validate_messages(good, expected), 3)
        variants = [[], good[:-1], good + good[:1], [message(*next(iter(SCOPE)), "", 401)]]
        variants += [[{**good[0], "responseBody": ""}, *good[1:]]]
        variants += [[{**good[0], "requestHeader": "GET http://elsewhere/ HTTP/1.1"}, *good[1:]]]
        for candidate in variants:
            with self.subTest(candidate=candidate), self.assertRaises(ValueError):
                validate_messages(candidate, expected)

    def test_anonymous_history_must_record_each_denial(self):
        anonymous = [
            {
                **message(host, path, "", 401),
                "requestHeader": f"GET http://{host}{path} HTTP/1.1\r\n",
            }
            for host, path in SCOPE
        ]
        validate_anonymous(anonymous)
        for variant in (
            [],
            anonymous[:-1],
            [anonymous[0]] * 3,
            [{**anonymous[0], "responseHeader": "HTTP/1.1 200 OK"}, *anonymous[1:]],
        ):
            with self.subTest(variant=variant), self.assertRaises(ValueError):
                validate_anonymous(variant)

    def test_zap_status_line_without_reason_and_malformed_controls(self):
        good = [message(host, path, marker) for (host, path), marker in SCOPE.items()]
        expected = {key: "synthetic" for key in SCOPE}
        no_reason = [{**item, "responseHeader": "HTTP/1.1 200\r\n"} for item in good]
        self.assertEqual(validate_messages(no_reason, expected), 3)
        anonymous = [
            {
                **item,
                "requestHeader": item["requestHeader"].split("\r\n")[0] + "\r\n",
                "responseHeader": "HTTP/1.1 401\r\n",
            }
            for item in good
        ]
        validate_anonymous(anonymous)
        for line in (
            "HTTP/1.1 4010",
            "HTTP/1.1 401x",
            "HTTP/1.0 401",
            "HTTP/1.1 200",
            "HTTP/1.1  401",
        ):
            with self.subTest(line=line), self.assertRaises(EvidenceError):
                validate_anonymous([{**anonymous[0], "responseHeader": line}, *anonymous[1:]])

    def test_auth_is_single_exact_value_with_case_insensitive_name(self):
        good = [message(host, path, marker) for (host, path), marker in SCOPE.items()]
        expected = {key: "synthetic" for key in SCOPE}
        good[0]["requestHeader"] = good[0]["requestHeader"].replace(
            "Authorization:", "aUtHoRiZaTiOn:"
        )
        self.assertEqual(validate_messages(good, expected), 3)
        for value in (
            "aUtHoRiZaTiOn: changed",
            "aUtHoRiZaTiOn: synthetic\r\nAuthorization: synthetic",
        ):
            bad = [
                {
                    **good[0],
                    "requestHeader": good[0]["requestHeader"].replace(
                        "aUtHoRiZaTiOn: synthetic", value
                    ),
                },
                *good[1:],
            ]
            with self.subTest(value=value), self.assertRaises(EvidenceError):
                validate_messages(bad, expected)

    def test_diagnostics_do_not_include_untrusted_values(self):
        host, path = next(iter(SCOPE))
        bad = message(host, path, "sensitive body", 403)
        with self.assertRaises(EvidenceError) as raised:
            validate_messages([bad], {key: "synthetic" for key in SCOPE})
        self.assertEqual(str(raised.exception), "bucket=response_status scope=1 status=403")
        with self.assertRaises(EvidenceError) as raised:
            validate_messages([{**bad, "requestHeader": "GET http://private-secret/ HTTP/1.1"}], {})
        self.assertEqual(str(raised.exception), "bucket=target scope=0 status=other")

    def test_history_categories_are_bounded_and_hide_untrusted_text(self):
        host, path = next(iter(SCOPE))
        proxied = {**message(host, path, "private body", 401), "type": "1"}
        foreign = {**proxied, "requestHeader": "GET http://secret.invalid/ HTTP/1.1", "type": "15"}
        result = history_summary([proxied, foreign] * 20)
        self.assertEqual(result["total"], "10plus")
        self.assertEqual(result["scope1"], "10plus")
        self.assertEqual(result["other_target"], "10plus")
        self.assertEqual(result["type_zap_user"], "10plus")
        self.assertNotIn("secret", str(result))
        self.assertNotIn("private", str(result))
        self.assertEqual(history_summary(None), {"shape": "invalid"})

    def test_rules_must_be_enabled_and_not_weakened(self):
        rule = {"enabled": "true", "alertThreshold": "DEFAULT"}
        self.assertEqual(validate_scanners([rule]), 1)
        for value in (
            [],
            [{**rule, "enabled": "false"}],
            [{**rule, "alertThreshold": "OFF"}],
            [{**rule, "alertThreshold": "HIGH"}],
        ):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_scanners(value)

    def test_alerts_fail_closed(self):
        self.assertEqual(validate_alerts([])["High"], 0)
        self.assertEqual(validate_alerts([{"pluginId": "10021", "risk": "Medium"}])["Medium"], 1)
        for alerts in [
            [{"pluginId": "1", "risk": "High"}],
            [{"pluginId": "1", "risk": "Critical"}],
            [{"pluginId": "x", "risk": "Low"}],
            None,
        ]:
            with self.subTest(alerts=alerts), self.assertRaises(ValueError):
                validate_alerts(alerts)


if __name__ == "__main__":
    unittest.main()
