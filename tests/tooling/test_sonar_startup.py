import unittest
from unittest.mock import patch

from sonar_ci import wait_ready


class Startup(unittest.TestCase):
    @patch("sonar_ci.time.sleep")
    @patch("sonar_ci.api")
    def test_reset_and_starting_are_not_mistaken_for_failed_analysis(self, api, sleep):
        api.side_effect = [ConnectionResetError(), {"status": "STARTING"}, {"status": "UP"}]
        wait_ready()
        self.assertEqual(api.call_count, 3)
        self.assertEqual(sleep.call_count, 2)

    @patch("sonar_ci.time.sleep")
    @patch("sonar_ci.time.monotonic", side_effect=[0, 0, 181])
    @patch("sonar_ci.api", return_value={"status": "STARTING"})
    def test_server_that_never_starts_fails_closed(self, api, clock, sleep):
        with self.assertRaisesRegex(ValueError, "did not become ready"):
            wait_ready()
        api.assert_called_once()
