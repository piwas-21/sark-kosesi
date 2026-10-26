import json
import os
import tempfile
import unittest
from contextlib import chdir
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError

from sonar_ci import verify
from sonar_policy import PROJECT
from test_sonar_policy import HEAD, fixture


class SonarVerification(unittest.TestCase):
    @patch("sonar_ci.print")
    @patch("sonar_ci.completed_analysis", return_value="analysis-123")
    @patch("sonar_ci.api")
    def test_verification_uses_provider_project_parameter_and_writes_current_evidence(
        self, api, completed, output
    ):
        identity, analyses, _, _, gate, measures, issues, hotspots = fixture()
        api.side_effect = [
            HTTPError("http://127.0.0.1", 404, "not found", {}, None),
            identity,
            analyses,
            gate,
            measures,
            issues,
            hotspots,
        ]
        with tempfile.TemporaryDirectory() as directory, chdir(directory):
            with patch.dict(
                os.environ, {"SONAR_TOKEN": "fixture-token", "ANALYSIS_REVISION": HEAD}
            ):
                verify()
            evidence = json.loads(Path(".local/sonar-evidence.json").read_text())
            self.assertEqual(evidence["revision"], HEAD)
            self.assertEqual(evidence["issues"], 0)
        api.assert_any_call(
            "api/hotspots/search", "fixture-token:", project=PROJECT, status="TO_REVIEW", ps=100
        )
        completed.assert_called_once_with("fixture-token:")
