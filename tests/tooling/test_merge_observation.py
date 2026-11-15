"""Offline controls for observing an ambiguous merge response."""

import json
import subprocess
import unittest
from unittest.mock import patch

from merge_gate import merge_once, observe_json, observed_exact_merge

REPO = "example/project"
NUMBER = 42
HEAD = "a" * 40
BASE = "b" * 40
MERGED = "c" * 40
TIP = "d" * 40
TREE = "e" * 40


def pull():
    return {
        "number": NUMBER,
        "state": "closed",
        "merged": True,
        "merged_at": "2026-01-01T00:00:00Z",
        "head": {"sha": HEAD, "ref": "codex/feature"},
        "base": {"ref": "develop"},
        "merge_commit_sha": MERGED,
    }


def commit(parents=None):
    return {
        "sha": MERGED,
        "parents": [{"sha": x} for x in (parents or [BASE])],
        "commit": {"tree": {"sha": TREE}},
    }


def ref(target=MERGED):
    return {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": target}}


def compare():
    return {
        "status": "ahead",
        "base_commit": {"sha": MERGED},
        "merge_base_commit": {"sha": MERGED},
    }


def observe(values, protected=False, head_ref="codex/feature"):
    with patch("merge_gate.observe_json", side_effect=values) as reader:
        with patch("merge_gate.reviewed_tree", return_value=TREE):
            result = observed_exact_merge(REPO, NUMBER, HEAD, BASE, "develop", head_ref, protected)
    return result, reader.call_args_list


class MergeObservation(unittest.TestCase):
    def test_exact_squash_and_advanced_target_are_confirmed(self):
        result, calls = observe([pull(), commit(), ref()])
        self.assertTrue(result)
        self.assertEqual(len(calls), 3)
        result, calls = observe([pull(), commit(), ref(TIP), compare()])
        self.assertTrue(result)
        self.assertEqual(calls[-2].args, (f"repos/{REPO}/git/ref/heads/develop",))
        self.assertEqual(calls[-1].args, (f"repos/{REPO}/compare/{MERGED}...{TIP}",))

    def test_release_requires_the_exact_two_commit_parents(self):
        pr = pull()
        pr["head"]["ref"] = "main"
        self.assertTrue(observe([pr, commit([BASE, HEAD]), ref()], True, "main")[0])
        self.assertFalse(observe([pr, commit(), ref()], True, "main")[0])

    def test_pr_head_target_parent_or_ancestry_mismatch_is_unresolved(self):
        variants = [
            {**pull(), "state": "open"},
            {**pull(), "merged": False},
            {**pull(), "merged_at": None},
            {**pull(), "number": NUMBER + 1},
            {**pull(), "head": {"sha": TIP, "ref": "codex/feature"}},
            {**pull(), "head": {"sha": HEAD, "ref": "codex/other"}},
            {**pull(), "base": {"ref": "main"}},
            {**pull(), "merge_commit_sha": "invalid"},
        ]
        for pr in variants:
            with self.subTest(pr=pr):
                self.assertFalse(observe([pr])[0])
        for value in (
            {"sha": TIP, "parents": [{"sha": BASE}]},
            commit([TIP]),
            commit([BASE, HEAD]),
            {**commit(), "commit": {"tree": {"sha": TIP}}},
            {**commit(), "commit": {"tree": {"sha": None}}},
        ):
            with self.subTest(commit=value):
                self.assertFalse(observe([pull(), value])[0])
        self.assertFalse(observe([pull(), commit(), {**ref(), "ref": "refs/heads/main"}])[0])
        self.assertFalse(
            observe([pull(), commit(), {**ref(), "object": {"type": "tag", "sha": TIP}}])[0]
        )
        self.assertFalse(
            observe([pull(), commit(), {**ref(), "object": {"type": "commit", "sha": "invalid"}}])[
                0
            ]
        )
        for value in (
            {**compare(), "status": "diverged"},
            {**compare(), "base_commit": {"sha": TIP}},
            {**compare(), "merge_base_commit": {"sha": BASE}},
            {"status": "ahead", "merge_base_commit": {"sha": MERGED}},
            {"status": "ahead", "base_commit": {"sha": MERGED}},
        ):
            with self.subTest(comparison=value):
                self.assertFalse(observe([pull(), commit(), ref(TIP), value])[0])

    def test_read_only_api_has_a_short_timeout(self):
        result = subprocess.CompletedProcess([], 0, stdout=json.dumps({"ok": True}), stderr="")
        with patch("merge_gate.subprocess.run", return_value=result) as run:
            self.assertEqual(observe_json("repos/example/project/pulls/42"), {"ok": True})
        self.assertEqual(run.call_args.args[0], ["gh", "api", "repos/example/project/pulls/42"])
        self.assertEqual(run.call_args.kwargs["timeout"], 5)
        self.assertTrue(run.call_args.kwargs["check"])

    def test_reviewed_tree_is_read_from_exact_local_commit_with_timeout(self):
        result = subprocess.CompletedProcess([], 0, stdout=TREE + "\n", stderr="")
        with patch("merge_gate.subprocess.run", return_value=result) as run:
            from merge_gate import reviewed_tree

            self.assertEqual(reviewed_tree(HEAD), TREE)
        self.assertEqual(
            run.call_args.args[0], ["git", "rev-parse", "--verify", f"{HEAD}^{{tree}}"]
        )
        self.assertEqual(run.call_args.kwargs["timeout"], 5)

    def test_failed_merge_never_retries_and_only_exact_observation_succeeds(self):
        command = ["gh", "pr", "merge", str(NUMBER), "--match-head-commit", HEAD, "--squash"]
        failed = subprocess.CalledProcessError(1, command)
        for observation in (False, subprocess.TimeoutExpired("gh", 5), OSError("offline")):
            with self.subTest(observation=observation):
                with patch("merge_gate.subprocess.run", side_effect=failed) as run:
                    with patch("merge_gate.observed_exact_merge", side_effect=[observation]):
                        with self.assertRaisesRegex(ValueError, "unresolved"):
                            merge_once(
                                command, REPO, NUMBER, HEAD, BASE, "develop", "codex/feature", False
                            )
                self.assertEqual(run.call_count, 1)
        with patch("merge_gate.subprocess.run", side_effect=failed) as run:
            with patch("merge_gate.observed_exact_merge", return_value=True):
                merge_once(command, REPO, NUMBER, HEAD, BASE, "develop", "codex/feature", False)
        self.assertEqual(run.call_count, 1)

    def test_successful_merge_does_not_invoke_observation(self):
        with patch("merge_gate.subprocess.run") as run:
            with patch("merge_gate.observed_exact_merge") as observer:
                merge_once(
                    ["gh", "pr", "merge"],
                    REPO,
                    NUMBER,
                    HEAD,
                    BASE,
                    "develop",
                    "codex/feature",
                    False,
                )
        self.assertEqual(run.call_count, 1)
        observer.assert_not_called()
