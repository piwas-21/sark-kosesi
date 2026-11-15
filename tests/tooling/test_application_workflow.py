"""Fail closed if the parallel application jobs lose required work or evidence."""

import json
import os
import re
import subprocess
import unittest
from pathlib import Path

WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
REVISION = "${{ github.event.pull_request.head.sha || github.sha }}"


def job(text, name):
    match = re.search(rf"(?ms)^  {re.escape(name)}:\n(.*?)(?=^  [a-z_]+:\n|\Z)", text)
    if not match:
        raise ValueError(f"missing job: {name}")
    return match.group(1)


def step(leg, marker):
    steps = re.split(r"(?m)^      - ", leg)
    matching = [item for item in steps if marker in item]
    if len(matching) != 1:
        raise ValueError(f"missing or duplicate step: {marker}")
    return matching[0]


def check_application_graph(text):
    checks, browser, coverage, gate, sonar, quality = (
        job(text, name)
        for name in (
            "application_checks",
            "application_browser",
            "application_coverage",
            "application",
            "sonar",
            "quality",
        )
    )
    for guarded in (checks, browser, coverage, gate):
        if re.search(r"(?m)^\s*(?:-\s+)?continue-on-error\s*:", guarded):
            raise ValueError("application work cannot continue on error")
    for leg in (checks, browser, coverage):
        for required in (
            f"ANALYSIS_REVISION: {REVISION}",
            f"ref: {REVISION}",
            "persist-credentials: false",
            "node-version-file: .nvmrc",
            "- run: npm ci --ignore-scripts",
            "- run: python3 scripts/install_app_tools.py",
        ):
            if required not in leg:
                raise ValueError(f"missing leg setup: {required}")
        if re.search(r"(?m)^    (if|needs):", leg):
            raise ValueError("application legs must run independently")
    for command in (
        "audit:rust",
        "typecheck",
        "test:unit",
        "test:integration",
        "test:migrations",
        "test:contracts",
    ):
        if len(re.findall(rf"(?m)^\s+(?:- )?run: npm run {re.escape(command)}$", checks)) != 1:
            raise ValueError(f"missing application command: {command}")
        if re.search(r"(?m)^(?:        )?if:", step(checks, f"run: npm run {command}\n")):
            raise ValueError(f"conditional application command: {command}")
    for command in ("check:rust", "test:coverage"):
        marker = f"run: npm run {command}\n"
        if len(re.findall(rf"(?m)^\s+(?:- )?run: npm run {re.escape(command)}$", coverage)) != 1:
            raise ValueError(f"missing coverage command: {command}")
        if re.search(r"(?m)^(?:        )?if:", step(coverage, marker)):
            raise ValueError(f"conditional coverage command: {command}")
        if marker in checks or marker in browser:
            raise ValueError(f"coverage command belongs to one leg: {command}")
    if coverage.index("run: npm run check:rust") > coverage.index("run: npm run test:coverage"):
        raise ValueError("Clippy must run before coverage validation")
    for line in (
        "rustup toolchain install nightly-2026-10-25 --profile minimal --component llvm-tools-preview",
        "if: failure() && steps.coverage.outcome == 'failure'",
        "name: rust-coverage-diagnostics-${{ env.ANALYSIS_REVISION }}",
        "name: application-coverage-${{ env.ANALYSIS_REVISION }}",
        "coverage/rust-stable.json",
        "coverage/rust.json",
        "coverage/rust.lcov",
        "coverage/clippy.json",
        "coverage/typescript/lcov.info",
        "if-no-files-found: error",
    ):
        if line not in coverage:
            raise ValueError(f"missing coverage evidence: {line}")
    for line in (
        "name: application-${{ env.ANALYSIS_REVISION }}",
        ".local/evidence/",
        "if-no-files-found: error",
    ):
        if line not in checks:
            raise ValueError(f"missing application evidence: {line}")
    coverage_upload = step(coverage, "name: application-coverage-${{ env.ANALYSIS_REVISION }}")
    if (
        "uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"
        not in coverage_upload
        or re.search(r"(?m)^(?:        )?if:", coverage_upload)
    ):
        raise ValueError("coverage evidence must upload after success")
    for path in (
        "coverage/rust-stable.json",
        "coverage/rust.json",
        "coverage/rust.lcov",
        "coverage/clippy.json",
        "coverage/typescript/lcov.info",
        "if-no-files-found: error",
    ):
        if path not in coverage_upload:
            raise ValueError(f"missing coverage upload: {path}")
    for line in (
        "- run: npm run build",
        "- run: npx playwright install --with-deps chromium firefox webkit",
        "run: npm run test:browser",
        "run: npm run test:guest-browser",
        "if: failure() && steps.browser.outcome == 'failure' && "
        "steps.browser.outputs.visual_candidates != 'complete'",
        "run: python3 scripts/with_postgres.py -- node tests/application/serve.mjs capture",
        "if: always()",
        "name: application-browser-${{ env.ANALYSIS_REVISION }}",
        "test-results/**/*.png",
        ".local/visual-candidates/",
        "if-no-files-found: error",
    ):
        if line not in browser:
            raise ValueError(f"missing browser evidence: {line}")
    application_upload = step(checks, "name: application-${{ env.ANALYSIS_REVISION }}")
    if re.search(r"(?m)^(?:        )?if:", application_upload):
        raise ValueError("application evidence upload cannot be conditional")
    for command in (
        "run: npm run build\n",
        "run: npm run test:browser\n",
        "run: npm run test:guest-browser\n",
    ):
        if re.search(r"(?m)^(?:        )?if:", step(browser, command)):
            raise ValueError(f"conditional browser command: {command}")
    browser_upload = step(browser, "name: application-browser-${{ env.ANALYSIS_REVISION }}")
    upload_conditions = re.findall(r"(?m)^(?:        )?if:\s*(.*?)\s*$", browser_upload)
    if upload_conditions != ["always()"]:
        raise ValueError("browser evidence must upload unconditionally after failure")
    gate_conditions = re.findall(r"(?m)^    if:\s*(.*?)\s*$", gate)
    if gate_conditions != ["always()"]:
        raise ValueError("application aggregate must always run")
    gate_step = step(gate, "name: Require all application legs")
    if re.search(r"(?m)^(?:        )?if:", gate_step):
        raise ValueError("application aggregate guard cannot be conditional")
    for line in (
        "name: Application gate",
        "if: always()",
        "needs: [application_checks, application_browser, application_coverage]",
        "RESULTS: ${{ toJSON(needs) }}",
    ):
        if line not in gate:
            raise ValueError(f"missing aggregate guard: {line}")
    coverage_download = step(sonar, "name: application-coverage-${{ env.ANALYSIS_REVISION }}")
    if (
        "uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333"
        not in coverage_download
        or "path: .local/application-coverage-artifact" not in coverage_download
        or re.search(r"(?m)^(?:        )?if:", coverage_download)
    ):
        raise ValueError("Sonar must download the exact coverage artifact")
    if "needs: [application]" not in sonar or "if: always()" not in sonar:
        raise ValueError("Sonar must depend on the aggregate")
    for line in (
        f"ANALYSIS_REVISION: {REVISION}",
        f"ref: {REVISION}",
        "name: application-coverage-${{ env.ANALYSIS_REVISION }}",
        "python3 scripts/check_coverage_reports.py",
        "cp .local/application-coverage-artifact/rust-stable.json coverage/rust-stable.json",
        "cp .local/application-coverage-artifact/rust.json coverage/rust.json",
        "cp .local/application-coverage-artifact/rust.lcov coverage/rust.lcov",
        "cp .local/application-coverage-artifact/clippy.json coverage/clippy.json",
        "cp .local/application-coverage-artifact/typescript/lcov.info coverage/typescript/lcov.info",
    ):
        if line not in sonar:
            raise ValueError(f"missing Sonar evidence binding: {line}")
    if "[foundation, secrets, dependencies, prototype, sast, application, sonar]" not in quality:
        raise ValueError("quality gate must include application and Sonar")


def aggregate_script(text):
    gate = job(text, "application")
    match = re.search(r"(?ms)^          python3 - <<'PY_GATE'\n(.*?)^          PY_GATE$", gate)
    if not match:
        raise ValueError("missing application aggregate program")
    return "\n".join(line.removeprefix("          ") for line in match.group(1).splitlines())


class ApplicationWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = WORKFLOW.read_text()

    def test_actual_graph_and_artifact_relationship_have_negative_controls(self):
        check_application_graph(self.workflow)
        for old, new in (
            (
                "needs: [application_checks, application_browser, application_coverage]",
                "needs: [application_checks]",
            ),
            ("ref: " + REVISION, "ref: develop"),
            ("  application_coverage:\n", "  coverage_missing:\n"),
            ("run: npm run check:rust", "run: echo skipped"),
            ("run: npm run test:coverage", "run: echo skipped"),
            ("name: application-coverage-${{ env.ANALYSIS_REVISION }}", "name: wrong-revision"),
            ("coverage/rust-stable.json", "coverage/missing.json"),
            (
                "uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9\n"
                "        with:\n          name: application-coverage-",
                "run: echo missing\n        with:\n          name: application-coverage-",
            ),
            ("path: .local/application-coverage-artifact", "path: .local/other"),
            (
                "name: application-coverage-${{ env.ANALYSIS_REVISION }}\n"
                "          path: .local/application-coverage-artifact",
                "name: application-coverage-wrong-revision\n"
                "          path: .local/application-coverage-artifact",
            ),
            ("coverage/clippy.json", "coverage/other.json"),
            ("name: application-${{ env.ANALYSIS_REVISION }}", "name: other-artifact"),
            (
                "- run: npm run test:integration",
                "- if: false\n        run: npm run test:integration",
            ),
            ("run: npm run test:browser", "run: echo skipped"),
            ("run: npm run test:guest-browser", "run: echo skipped"),
            ("name: application-browser-${{ env.ANALYSIS_REVISION }}", "name: other-browser"),
            ("needs: [application]", "needs: [application_checks]"),
            ("python3 scripts/check_coverage_reports.py", "echo skipped"),
        ):
            with self.subTest(old=old), self.assertRaises(ValueError):
                check_application_graph(self.workflow.replace(old, new, 1))

    def test_required_step_conditions_cannot_be_disabled(self):
        mutations = (
            (
                "  application:\n    name: Application gate\n    if: always()",
                "  application:\n    name: Application gate\n    if: always() && false",
            ),
            (
                "      - name: Require all application legs\n",
                "      - name: Require all application legs\n        if: false\n",
            ),
            (
                "if: always()\n        with:\n          name: application-browser-",
                "if: always() && false\n        with:\n          name: application-browser-",
            ),
            ("      - run: npm run build\n", "      - if: false\n        run: npm run build\n"),
            ("      - id: browser\n", "      - id: browser\n        if: false\n"),
            (
                "      - name: Exercise private guest checkout in Chromium\n",
                "      - name: Exercise private guest checkout in Chromium\n        if: false\n",
            ),
            ("      - id: coverage\n", "      - id: coverage\n        if: false\n"),
        )
        for old, new in mutations:
            with self.subTest(old=old), self.assertRaises(ValueError):
                self.assertIn(old, self.workflow)
                check_application_graph(self.workflow.replace(old, new, 1))

    def test_no_application_job_or_step_can_continue_on_error(self):
        mutations = (
            ("  application_checks:\n", "  application_checks:\n    continue-on-error: true\n"),
            (
                "  application_browser:\n",
                "  application_browser:\n    continue-on-error: ${{ true }}\n",
            ),
            ("  application_coverage:\n", "  application_coverage:\n    continue-on-error: true\n"),
            ("  application:\n", "  application:\n    continue-on-error: true\n"),
            ("      - id: coverage\n", "      - id: coverage\n        continue-on-error: true\n"),
            (
                "      - id: browser\n",
                "      - id: browser\n        continue-on-error: ${{ true }}\n",
            ),
            (
                "      - name: Require all application legs\n",
                "      - name: Require all application legs\n        continue-on-error: true\n",
            ),
        )
        for old, new in mutations:
            with self.subTest(old=old), self.assertRaisesRegex(ValueError, "continue on error"):
                check_application_graph(self.workflow.replace(old, new, 1))

    def test_actual_aggregate_requires_all_successes(self):
        program = aggregate_script(self.workflow)
        good = {
            name: {"result": "success"}
            for name in ("application_checks", "application_browser", "application_coverage")
        }

        def result(value):
            return subprocess.run(
                ["python3", "-c", program],
                env={**os.environ, "RESULTS": json.dumps(value)},
                capture_output=True,
                text=True,
                check=False,
            ).returncode

        self.assertEqual(result(good), 0)
        for name in good:
            for state in ("failure", "cancelled", "skipped", "", None):
                with self.subTest(name=name, state=state):
                    value = {key: dict(entry) for key, entry in good.items()}
                    value[name]["result"] = state
                    self.assertNotEqual(result(value), 0)
        for name in good:
            missing = {key: value for key, value in good.items() if key != name}
            with self.subTest(missing=name):
                self.assertNotEqual(result(missing), 0)
        for value in ({}, {"application_checks": {"result": "success"}}, [], None):
            with self.subTest(value=value):
                self.assertNotEqual(result(value), 0)
