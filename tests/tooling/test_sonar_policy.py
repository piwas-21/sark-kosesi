import copy
import unittest

from sonar_policy import PROJECT, validate_analysis, validate_evidence

HEAD = "a" * 40


def fixture():
    return [
        {"component": {"key": PROJECT}},
        {"analyses": [{"key": "analysis-123", "revision": HEAD}]},
        "analysis-123",
        HEAD,
        {
            "projectStatus": {
                "status": "OK",
                "conditions": [{"metricKey": "reliability_rating", "status": "OK"}],
            }
        },
        {"component": {"key": PROJECT, "measures": [{"metric": "ncloc", "value": "123"}]}},
        {"paging": {"total": 0}, "issues": []},
        {"paging": {"total": 0}, "hotspots": []},
    ]


class SonarPolicy(unittest.TestCase):
    def test_populated_current_analysis(self):
        evidence = validate_analysis(*fixture())
        self.assertEqual(evidence["ncloc"], 123)
        self.assertEqual(evidence["revision"], HEAD)
        validate_evidence(evidence, HEAD)

    def test_wrong_scope_empty_and_stale_analysis(self):
        mutations = [
            (0, {"component": {"key": "wrong"}}),
            (1, {"analyses": []}),
            (1, {"analyses": [{"key": "analysis-123", "revision": "b" * 40}]}),
            (2, "different-task"),
            (4, {"projectStatus": {"status": "OK", "conditions": []}}),
            (4, {"projectStatus": {"status": "ERROR", "conditions": [{"status": "ERROR"}]}}),
            (4, {"projectStatus": {"status": "OK", "conditions": [{"status": "NONE"}]}}),
            (5, {"component": {"key": PROJECT, "measures": []}}),
            (5, {"component": {"key": "wrong", "measures": [{"metric": "ncloc", "value": "123"}]}}),
            (6, {}),
            (6, {"errors": ["no project"], "paging": {"total": 0}, "issues": []}),
            (6, {"paging": {"total": 1}, "issues": [{"message": "bug"}]}),
            (7, {}),
            (7, {"paging": {"total": 1}, "hotspots": [{"message": "review"}]}),
        ]
        for index, replacement in mutations:
            values = fixture()
            values[index] = replacement
            with self.subTest(index=index, replacement=replacement), self.assertRaises(ValueError):
                validate_analysis(*values)

    def test_artifact_missing_fields_and_stale_revision(self):
        evidence = validate_analysis(*fixture())
        for key in evidence:
            modified = copy.deepcopy(evidence)
            del modified[key]
            with self.subTest(key=key), self.assertRaises(ValueError):
                validate_evidence(modified, HEAD)
        for change in (
            {"revision": "b" * 40},
            {"project": "wrong"},
            {"ncloc": 0},
            {"ncloc": "123"},
            {"conditions": [{"status": "ERROR"}]},
            {"issues": 1},
            {"hotspots": 1},
        ):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_evidence({**evidence, **change}, HEAD)
