"""Offline controls for scanner evidence, not a substitute for the hosted scan."""

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "passive-preview"))
from evidence import (
    SCOPE,
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
