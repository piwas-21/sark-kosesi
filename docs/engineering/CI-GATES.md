# CI and enforcement matrix

## Implemented foundation

Required contexts are configured in `config/delivery.json` and mirrored in `config/github-ruleset.json`. The GitHub `main-develop` ruleset is active with no bypass actors, no deletion/force push, PR requirement, resolved conversations and up-to-date required checks. The repository is public. Audit live settings rather than trusting this prose after a visibility change.

| Check | Actual scope | Failure behavior |
| --- | --- | --- |
| Foundation | Required docs, local links/assets, asset hashes, portable paths, pinned workflow actions, prototype JS parsing, tooling unit tests, branch/PR policy | Missing or inconsistent content fails |
| Secret scan | Pinned Gitleaks CLI over Git history and working files | Any detected secret or scanner error fails; redacted output |
| Dependency audit | npm lockfile audit including dev dependencies | High/critical findings or audit-service failure fail |
| Prototype smoke | Real Chromium, seven-locale width, device preferences, RTL, covers basket, fit failure, reduced motion, product accessibility | Assertion failure, browser errors, zero tests or unexpected external request fails |
| CodeQL (javascript-typescript) | GitHub CodeQL analysis of available JS/TS | Query/build/upload failure fails; production security triage also checks alerts |
| CodeQL (python) | CodeQL analysis of repository tooling | Query/build/upload failure fails; not a full commerce audit |
| Quality gate | All required jobs must report success | Failed, cancelled or skipped prerequisite fails |

The browser job uses the official Playwright container pinned by digest and matched to the npm package version; update both together. Its root browser is restricted to the local synthetic fixture by the test network guard and has no credentials or deployment access. See the [upstream container guidance](https://playwright.dev/docs/docker). Actions are pinned to immutable commits, tokens have per-job least privilege, checkout credentials are not persisted, untrusted PR text is not interpolated into shell, and all jobs have timeouts. PR CI has no production secrets or deployment access. No `pull_request_target` or privileged untrusted checkout. Dependabot proposes dependency/action updates. GitHub secret scanning/push protection and Dependabot security updates are enabled; availability must be rechecked after a privacy/plan change.

CodeQL success means analysis ran; it does not mean no alerts. The merge gate also checks current open code-scanning alerts in the PR scope and fails on unresolved results. Security high/critical thresholds are not silently downgraded. SonarCloud is not configured or claimed here; CodeQL and review are the initial SAST/review combination.

## Application CI required at P1

The foundation guard rejects production files under `apps/` while `stage` is `foundation`. Change stage to `application` only in the PR that installs real application commands/jobs and updates required contexts/rules. A stage string alone is not enough; the guard requires named application scripts and jobs. Inspect the actual job commands in review; never add echo/empty placeholder passes.

| Required production job | Acceptance |
| --- | --- |
| Typecheck + lint | Strict TS; zero lint warnings; naming/module/file-size rules |
| Production build | Actual app builds with validated synthetic configuration, no live resources or build-time secret fetch |
| Unit/domain | Money/discount/tax allocation, state transitions, locale and fit predicates; meaningful branch coverage |
| Integration/contracts | Isolated PostgreSQL and workers; stock/payment concurrency, ownership and provider adapters |
| Migration | Fresh database + previous-release schema/data upgrade, schema drift, backwards-compatible rolling rollout |
| E2E | Guest/account checkout, return/reconcile, admin roles, cancellation/refund, messages/alerts/reviews; Chromium/WebKit/Firefox core flows |
| Accessibility | Axe serious/critical zero on core flows; keyboard/zoom/RTL manual release checklist |
| Discovery | Served SSR content, expected sitemap inventory, canonical/hreflang/locale/schema/feed and noindex contracts |
| Performance | Pinned production build, payload budgets and 3-run mobile lab median; realistic API/stock load |
| Supply chain + container | Audit/SAST/secrets, SPDX licence policy, SBOM and image scan of the built image, non-root/runtime controls |
| Release gate | Source SHA, current code-scan triage, required manual/provider/legal evidence, restore/rollback and protected branches |

Required commands: `typecheck`, `lint`, `build`, `test:unit`, `test:integration`, `test:migrations`, `test:e2e`, `test:accessibility`, `test:discovery`, `test:performance`. They must execute tests against the app, not the prototype. Wire their jobs into Quality gate and GitHub rules before merging production app files. `--passWithNoTests`, disabled/skipped required suites and `continue-on-error` are prohibited.

## Before merge and release

Local: clean owned diff, checks, independent review receipt for current head/base. Remote: every named check exists and succeeds, no unresolved conversations/changes-requested review, no open PR code-scanning alerts, current base ancestry and active rules. A same-named status from another commit does not count.

The production gate currently fails deliberately because there is no production app or acceptance evidence. Activating `productionEnabled` requires `.local/releases/<head-sha>/evidence.json` bound to the exact release head, each declared gate marked passed with a repository-relative evidence path, and server protection audit. Evidence content still requires human review; a JSON field is not proof by itself.

On CI failure, read full relevant logs and exit status. Do not infer success from the last two lines or an empty API response. CodeQL/secret/audit scans need a populated analysed scope. Scheduled scans detect newly disclosed dependency/code issues on the default branch (develop); they do not replace PR checks.
