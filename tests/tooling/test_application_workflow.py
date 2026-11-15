"""Fail closed if the parallel application jobs lose required work or evidence."""

import json
import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml"
PACKAGE = Path(__file__).resolve().parents[2] / "package.json"
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
    checks, browser, stable, nightly, coverage, gate, sonar, quality = (
        job(text, name)
        for name in (
            "application_checks",
            "application_browser",
            "coverage_stable",
            "coverage_nightly",
            "application_coverage",
            "application",
            "sonar",
            "quality",
        )
    )
    for guarded in (checks, browser, stable, nightly, coverage, gate):
        if re.search(r"(?m)^\s*(?:-\s+)?continue-on-error\s*:", guarded):
            raise ValueError("application work cannot continue on error")
    for leg in (checks, browser, stable, nightly):
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
    for leg, commands in (
        (stable, ("npm run check:rust", "npm run test:coverage:stable")),
        (nightly, ("npm run test:coverage:nightly", "npm run test:coverage:typescript")),
    ):
        for command in commands:
            if len(re.findall(rf"(?m)^\s+(?:- )?(?:run: )?{re.escape(command)}$", leg)) != 1:
                raise ValueError(f"missing coverage command: {command}")
        if re.search(r"(?m)^(?:        )?if:", step(leg, "id: coverage\n")):
            raise ValueError("coverage producer cannot be conditional")
        if "if: failure() && steps.coverage.outcome == 'failure'" not in leg:
            raise ValueError("missing failure diagnostics")
    if re.search(r"(?m)^(?:        )?if:", step(stable, "run: npm run check:rust\n")):
        raise ValueError("Clippy cannot be conditional")
    if stable.index("run: npm run check:rust") > stable.index("id: coverage"):
        raise ValueError("Clippy must precede the stable coverage producer")
    if step(stable, "id: coverage\n").strip() != (
        "id: coverage\n        run: npm run test:coverage:stable"
    ) or step(nightly, "id: coverage\n").strip() != (
        "id: coverage\n        run: |\n"
        "          npm run test:coverage:nightly\n"
        "          npm run test:coverage:typescript"
    ):
        raise ValueError("coverage producer commands must run directly")
    if (
        "rustup toolchain install nightly-2026-10-25 --profile minimal --component llvm-tools-preview"
        not in nightly
    ):
        raise ValueError("nightly coverage toolchain missing")
    for leg, artifact, paths in (
        (stable, "coverage-stable-", ("coverage/rust-stable.json", "coverage/clippy.json")),
        (
            nightly,
            "coverage-nightly-",
            ("coverage/rust.json", "coverage/rust.lcov", "coverage/typescript/lcov.info"),
        ),
    ):
        upload = step(leg, f"name: {artifact}${{{{ env.ANALYSIS_REVISION }}}}")
        if (
            "uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9" not in upload
            or re.search(r"(?m)^(?:        )?if:", upload)
            or "if-no-files-found: error" not in upload
            or any(path not in upload for path in paths)
        ):
            raise ValueError("coverage producer upload is incomplete or conditional")
    for leg, name, paths in (
        (
            stable,
            "rust-stable-coverage-diagnostics-",
            ("coverage/rust-stable.json", "coverage/clippy.json"),
        ),
        (nightly, "rust-coverage-diagnostics-", ("coverage/rust.json", "coverage/rust.lcov")),
    ):
        diagnostic = step(leg, f"name: {name}${{{{ env.ANALYSIS_REVISION }}}}")
        if (
            "uses: actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9"
            not in diagnostic
            or "if: failure() && steps.coverage.outcome == 'failure'" not in diagnostic
            or "if-no-files-found: ignore" not in diagnostic
            or any(path not in diagnostic for path in paths)
        ):
            raise ValueError("coverage failure diagnostics missing")
    if (
        "needs: [coverage_stable, coverage_nightly]" not in coverage
        or re.search(r"(?m)^    if:", coverage)
        or f"ANALYSIS_REVISION: {REVISION}" not in coverage
        or f"ref: {REVISION}" not in coverage
        or "persist-credentials: false" not in coverage
    ):
        raise ValueError("coverage aggregation must depend on both exact-revision producers")
    for artifact, path in (
        ("coverage-stable-", ".local/coverage-stable-artifact"),
        ("coverage-nightly-", ".local/coverage-nightly-artifact"),
    ):
        download = step(coverage, f"name: {artifact}${{{{ env.ANALYSIS_REVISION }}}}")
        if (
            "uses: actions/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333"
            not in download
            or f"path: {path}" not in download
            or re.search(r"(?m)^(?:        )?if:", download)
            or re.search(r"(?m)^\s+(run-id|repository|github-token):", download)
        ):
            raise ValueError("coverage producer artifact missing or unbound")
    validation = step(coverage, "name: Validate complete coverage")
    for line in (
        "cp .local/coverage-stable-artifact/rust-stable.json coverage/rust-stable.json",
        "cp .local/coverage-stable-artifact/clippy.json coverage/clippy.json",
        "cp .local/coverage-nightly-artifact/rust.json coverage/rust.json",
        "cp .local/coverage-nightly-artifact/rust.lcov coverage/rust.lcov",
        "cp .local/coverage-nightly-artifact/typescript/lcov.info coverage/typescript/lcov.info",
        "python3 scripts/check_coverage_reports.py",
    ):
        if line not in validation:
            raise ValueError(f"missing aggregate evidence check: {line}")
    if re.search(r"(?m)^(?:        )?if:", validation):
        raise ValueError("coverage validation cannot be conditional")
    if coverage.index("name: Validate complete coverage") > coverage.index(
        "name: application-coverage-${{ env.ANALYSIS_REVISION }}"
    ):
        raise ValueError("canonical upload must follow validation")
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


def coverage_script(text):
    coverage = job(text, "application_coverage")
    match = re.search(
        r"(?ms)^      - name: Validate complete coverage\n        run: \|\n(.*?)(?=^      - |\Z)",
        coverage,
    )
    if not match:
        raise ValueError("missing coverage validation program")
    return "\n".join(line.removeprefix("          ") for line in match.group(1).splitlines())


def check_coverage_commands(scripts):
    expected = {
        "test:coverage:stable": "CARGO_TARGET_DIR=.local/coverage-target-stable cargo llvm-cov clean --workspace && CARGO_TARGET_DIR=.local/coverage-target-stable python3 scripts/with_postgres.py -- cargo llvm-cov --workspace --all-features --locked --json --output-path coverage/rust-stable.json",
        "test:coverage:nightly": "CARGO_TARGET_DIR=.local/coverage-target-nightly cargo +nightly-2026-10-25 llvm-cov clean --workspace && CARGO_TARGET_DIR=.local/coverage-target-nightly python3 scripts/with_postgres.py -- cargo +nightly-2026-10-25 llvm-cov --workspace --all-features --locked --branch --json --output-path coverage/rust.json && CARGO_TARGET_DIR=.local/coverage-target-nightly cargo +nightly-2026-10-25 llvm-cov report --lcov --output-path coverage/rust.lcov",
        "test:coverage:typescript": "vitest run --coverage",
        "test:coverage": "npm run test:coverage:stable && npm run test:coverage:nightly && npm run test:coverage:typescript && python3 scripts/check_coverage_reports.py",
    }
    if any(scripts.get(name) != command for name, command in expected.items()):
        raise ValueError("coverage command or local composition changed")


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
            ("run: npm run test:coverage:stable", "run: echo skipped"),
            ("npm run test:coverage:nightly", "echo skipped"),
            ("npm run test:coverage:typescript", "echo skipped"),
            ("needs: [coverage_stable, coverage_nightly]", "needs: [coverage_stable]"),
            ("name: coverage-stable-${{ env.ANALYSIS_REVISION }}", "name: wrong-stable"),
            ("name: coverage-nightly-${{ env.ANALYSIS_REVISION }}", "name: wrong-nightly"),
            (
                "path: .local/coverage-stable-artifact",
                "path: .local/coverage-stable-artifact\n          run-id: 123",
            ),
            (
                "name: rust-coverage-diagnostics-${{ env.ANALYSIS_REVISION }}",
                "name: missing-diagnostics",
            ),
            ("name: application-coverage-${{ env.ANALYSIS_REVISION }}", "name: wrong-revision"),
            ("cp .local/coverage-stable-artifact/rust-stable.json", "cp missing.json"),
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
            ("cp .local/coverage-stable-artifact/clippy.json", "cp other.json"),
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

    def test_factored_coverage_commands_preserve_the_full_local_suite(self):
        scripts = json.loads(PACKAGE.read_text())["scripts"]
        check_coverage_commands(scripts)
        for name in (
            "test:coverage:stable",
            "test:coverage:nightly",
            "test:coverage:typescript",
            "test:coverage",
        ):
            with self.subTest(name=name), self.assertRaises(ValueError):
                check_coverage_commands({**scripts, name: "echo skipped"})

    def test_coverage_assembly_fails_for_each_missing_file_and_failed_validation(self):
        program = coverage_script(self.workflow)
        inputs = (
            ".local/coverage-stable-artifact/rust-stable.json",
            ".local/coverage-stable-artifact/clippy.json",
            ".local/coverage-nightly-artifact/rust.json",
            ".local/coverage-nightly-artifact/rust.lcov",
            ".local/coverage-nightly-artifact/typescript/lcov.info",
        )

        def run(missing=None, fail=False):
            with tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                for name in inputs:
                    if name != missing:
                        path = root / name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_text("synthetic input")
                script = root / "scripts/check_coverage_reports.py"
                script.parent.mkdir()
                script.write_text(
                    "from pathlib import Path\n"
                    "Path('validator-ran').write_text('yes')\n"
                    f"raise SystemExit({int(fail)})\n"
                )
                result = subprocess.run(
                    ["bash", "-e", "-o", "pipefail", "-c", program],
                    cwd=root,
                    capture_output=True,
                    text=True,
                    check=False,
                )
                return result.returncode, (root / "validator-ran").exists()

        self.assertEqual(run(), (0, True))
        self.assertEqual(run(fail=True), (1, True))
        for name in inputs:
            with self.subTest(missing=name):
                code, called = run(missing=name)
                self.assertNotEqual(code, 0)
                self.assertFalse(called)

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
            (
                "  application_coverage:\n    name: Application coverage\n",
                "  application_coverage:\n    name: Application coverage\n    if: always()\n",
            ),
            (
                "      - name: Validate complete coverage\n",
                "      - name: Validate complete coverage\n        if: false\n",
            ),
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
            ("  coverage_stable:\n", "  coverage_stable:\n    continue-on-error: true\n"),
            ("  coverage_nightly:\n", "  coverage_nightly:\n    continue-on-error: true\n"),
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
