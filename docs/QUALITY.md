# Quality and acceptance

The workflow and `config/delivery.json` are the executable foundation inventory. Missing, skipped, cancelled, stale or empty required results fail closed. A foundation green result is not an application test, accessibility certification, Rust audit or release approval.

## Enforced foundation checks

| Check            | Evidence                                                                                                                                                        |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Foundation       | Portable links/assets, pinned actions/images, branch/stage contract, Python and JS lint/format, hostile validation tests and coverage                           |
| Secret scan      | Pinned Gitleaks over tracked history and current source, redacted output                                                                                        |
| Dependency audit | Locked npm audit for high/critical advisories                                                                                                                   |
| Prototype smoke  | Pinned Chromium/Playwright, seven locales, device defaults, configuration, room dimensions, keyboard/reduced motion and axe                                     |
| CodeQL JS/Python | Current populated analysis and no open PR alerts; additional public-repository scanning                                                                         |
| SonarQube        | Disposable Community Build, populated exact-revision analysis, quality conditions, zero unresolved issues and zero outstanding hotspots; JSON evidence artifact |
| Quality gate     | Every prerequisite succeeded; no optional or skipped checks                                                                                                     |

`npm run check` runs format/lint, foundation contracts and tooling coverage. Python tools are hash-pinned in `requirements-dev.txt`; JavaScript tools in the npm lockfile. Coverage is a minimum 90% statements/branches combined for the pure gate-policy modules (`gate_policy`, `review_policy`, `sonar_policy`, `check_app_coverage`); wrapper/network/tooling execution is separately exercised by CI and negative-control tests. Do not call this whole-application coverage. Prototype source is a reference design, checked for syntax/assets/browser behavior; production lint, coverage and size budgets cannot exempt production code by moving it under design.

## Sonar ownership and operation

Use free, self-hostable SonarQube Community Build, pinned by image digest. Each CI job runs a loopback-only temporary server with disposable data, bootstraps random credentials, scans the candidate commit and validates the analysis before publishing evidence. No repository Sonar secret, external SonarCloud subscription or paid PR-analysis feature is required. This uses ordinary main-project analysis for each candidate; there is no persistent PR dashboard, trend history or paid PR decoration. The GitHub check and downloadable evidence are the review surface.

The gate verifies project identity and the analysis revision, populated metrics and quality conditions, no unresolved issues and no outstanding security hotspots. It also probes a nonexistent project to expose misleading empty API results. Coverage generation is a separate fail-closed job step; Sonar imports it rather than inventing coverage. Current scan scope is owned Python tooling and maintained JS test/configuration source; the non-production design specimen is excluded from this maintainability scan, not from secret scanning/browser checks. P1 expands the explicit scope to every app/crate/package and imports real Rust/TS coverage and Clippy JSON. Sonar's Rust support imports Clippy and complexity/coverage metrics; it is not a comprehensive Rust security audit.

H2/disposable storage is for isolated CI only. A future persistent self-hosted dashboard requires a supported PostgreSQL instance, TLS/access control, backups and explicit maintenance ownership. Do not deploy the CI server to production or expose its bootstrap credentials. Official references: [Community Rust analysis](https://docs.sonarsource.com/sonarqube-community-build/analyzing-source-code/languages/rust), [Linux prerequisites](https://docs.sonarsource.com/sonarqube-community-build/server-installation/pre-installation/linux).

## Application gates to activate in P1

No fake commands or passing jobs before code exists. Pin the following free tools when introduced, record licence/version in lockfiles and require their real outputs:

| Command                                      | Required behavior                                                                                                                                                                                             |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `npm run check:rust`                         | `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`; forbid unsafe in owned crates                                                                  |
| `npm run audit:rust`                         | cargo-audit against a current advisory DB; cargo-deny for licences/sources/bans; fail on stale/missing database or report                                                                                     |
| `npm run typecheck` / `lint` / `build`       | Strict TS, ESLint, style checks, Next production build and Cargo release build; no ignored errors                                                                                                             |
| `npm run test:unit`                          | Rust domain and TS component/state tests; independent oracle values, edge and property tests                                                                                                                  |
| `npm run test:coverage`                      | cargo-llvm-cov and Vitest coverage; >=85% lines and >=80% branches per maintained app/domain package, critical money/inventory/authorization >=95%; exclusions only generated code and reviewed nonlogic glue |
| `npm run test:integration`                   | Real ephemeral PostgreSQL, constraints/locking/outbox/provider-adapter contracts, concurrent and failure tests                                                                                                |
| `npm run test:migrations`                    | Empty database and previous supported schema → current; data preservation and expand/contract rollback compatibility                                                                                          |
| `npm run test:contracts`                     | Generated OpenAPI/client diff, HTTP fixtures and version compatibility                                                                                                                                        |
| `npm run test:e2e`                           | Chromium/Firefox/WebKit critical journeys with isolated mail/provider fixtures; no uncontrolled external traffic                                                                                              |
| `npm run test:accessibility` / `test:visual` | axe plus keyboard and approved gallery/journey screenshots; all locales, themes, responsive/RTL/error/loading states                                                                                          |
| `npm run test:security`                      | Negative RBAC/ownership/CSRF/abuse/upload/cache tests, secret/dependency scan and isolated authenticated dynamic scan                                                                                         |
| `npm run test:discovery`                     | Served HTML, sitemap, canonical/hreflang, product/feed and negative controls in DISCOVERY                                                                                                                     |
| `npm run test:performance`                   | Production-build Lighthouse CI and payload budgets; k6 isolated API/load fixture in performance config                                                                                                        |

Rust line/region coverage runs on the pinned stable release. Rust branch instrumentation currently requires nightly in cargo-llvm-cov: P1 must pin a separate tested nightly toolchain for the branch-coverage job and run `cargo +<pinned-nightly> llvm-cov --branch`. Production builds remain on stable. Missing branch data fails the coverage gate; do not report it as 100% or silently substitute line coverage. See the [upstream coverage requirements](https://github.com/taiki-e/cargo-llvm-cov#readme).

`scripts/check_app_coverage.py --report REPORT.lcov --package OWNED_SOURCE_DIRECTORY`
validates LCOV from the actual coverage generators. Invoke it separately for each
maintained package source directory, and add `--critical SOURCE_FILE` for each
money/inventory/authorization module. It requires every source in that directory
in the report, checks line/branch records against their totals and actual source
locations, and enforces 85% lines / 80% branches (95% / 95% for critical modules).
Missing branches fail even when line coverage is full. Generated code belongs in
a separate generated directory; exclusions require review rather than hidden
filename filters. CI must generate reports on the candidate revision before
calling this validator. Its hostile fixture tests prove the validator only;
application coverage remains pending until actual app/crate reports pass.

Critical cases: last shared foam component contested by two orders; covers-only reserves no foam; payment-create timeout and late paid after expiry; duplicated/reordered webhooks; partial refund/tax/discount rounding; coupon exhaustion; expired guest token; cross-customer access; worker crash after provider acceptance; duplicate restock mail; incomplete translation/private-cache leak. Mutation testing on financial/authorization invariants must demonstrate that the test detects the intended defect, not just a textual mutation.

## Accessibility and performance acceptance

Target WCAG 2.2 AA. Automated checks require zero serious/critical violations and triage all lower findings; manual review covers complete journeys with keyboard only, VoiceOver/Safari, NVDA/Firefox, 200/400% zoom, 320px reflow, high contrast, reduced motion and Arabic RTL. Check focus restoration, error summary, accessible names/live regions, contrast in every state and touch targets. Planner dragging has keyboard/input alternatives; 3D never hides the only dimensions. Native speakers review seven locales and fonts. Automated scores do not certify compliance.

Budgets are in `config/performance-budgets.json`: field p75 LCP <=2.5s, INP <=200ms, CLS <=0.1; lab median of three controlled runs Lighthouse >=90, LCP <=2.5s, TBT <=200ms; initial first-party JS <=170KiB gzip, mobile LCP image <=250KiB, fonts <=150KiB, total <=750KiB. Test home/product/cart/checkout/account and planner-open separately. Lazy-load room tools and nonessential motion; responsive images with dimensions, self-host subset fonts and cache-safe SSR. Never score the unoptimized specimen as production.

At 25 concurrent users for 10 minutes plus a 50-request burst, catalogue p95 <=300ms and quote p95 <=500ms excluding separately reported provider time, errors <=1%; no overselling, leaked resources or unbounded queue growth. Record hardware, DB size, seed, cache state and exact SHA. Resource caps and profiling matter more than language claims. P6 captures lab evidence; post-launch field data validates real-user performance separately.
