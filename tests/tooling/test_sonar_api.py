import io
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError

from sonar_ci import api, bootstrap


class SonarApi(unittest.TestCase):
    @patch("sonar_ci.urlopen")
    def test_http_diagnostics_do_not_reveal_credentials_or_form_secrets(self, open_url):
        error = HTTPError(
            "http://127.0.0.1:9000/api/example",
            400,
            "bad request",
            {},
            io.BytesIO(b"invalid admin:private-token and new-private-value"),
        )
        open_url.side_effect = error
        with self.assertRaises(HTTPError) as raised:
            api("api/example", "admin:private-token", data={"password": "new-private-value"})
        diagnostic = " ".join(raised.exception.__notes__)
        self.assertIn("[redacted]", diagnostic)
        self.assertNotIn("private-token", diagnostic)
        self.assertNotIn("new-private-value", diagnostic)

    @patch("sonar_ci.print")
    @patch("sonar_ci.secrets.token_urlsafe", return_value="letterswithoutdigits")
    @patch("sonar_ci.wait_ready")
    @patch("sonar_ci.api")
    def test_bootstrap_guarantees_password_classes_without_relying_on_random_chance(
        self, call, ready, random, output
    ):
        call.side_effect = [{}, {}, {"name": "gate"}, {}, {}, {}, {}, {"token": "test-token"}]
        with tempfile.TemporaryDirectory() as directory:
            env_file = Path(directory) / "environment"
            with patch.dict(os.environ, {"GITHUB_ENV": str(env_file)}):
                bootstrap()
            password = call.call_args_list[0].kwargs["data"]["password"]
            for pattern in ("[A-Z]", "[a-z]", "[0-9]", "[^a-zA-Z0-9]"):
                self.assertRegex(password, pattern)
            self.assertGreaterEqual(len(password), 12)
            self.assertIn("SONAR_TOKEN=test-token", env_file.read_text())
            ready.assert_called_once()
            random.assert_called_once_with(36)
            output.assert_any_call("::add-mask::test-token", flush=True)
