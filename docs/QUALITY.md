# Quality and acceptance

The workflow and `config/delivery.json` are the executable foundation inventory. Missing, skipped, cancelled, stale or empty required results fail closed. A foundation green result is not an application test, accessibility certification, Rust audit or release approval.

## Enforced foundation checks

| Check            | Evidence                                                                                                                                                        |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Foundation       | Portable links/assets, pinned actions/images, branch/stage contract, Python and JS lint/format, hostile validation tests and coverage                           |
| Secret scan      | Pinned Gitleaks over tracked history and Git-owned current source, redacted output and seeded detector control                                                  |
| Dependency audit | Locked npm audit for high/critical advisories                                                                                                                   |
| Prototype smoke  | Pinned Chromium/Playwright, seven locales, device defaults, configuration, room dimensions, keyboard/reduced motion and axe                                     |
| CodeQL JS/Python | Current populated analysis and no open PR alerts; additional public-repository scanning                                                                         |
| SonarQube        | Disposable Community Build, populated exact-revision analysis, quality conditions, zero unresolved issues and zero outstanding hotspots; JSON evidence artifact |
| Quality gate     | Every prerequisite succeeded; no optional or skipped checks                                                                                                     |

The current-source secret scan exports tracked and non-ignored untracked files.
Ignored framework build/runtime keys are not source; force-tracked ignored files
remain scanned, and the complete tracked history scan still runs. A generated
fake credential must be detected before accepting the clean source result.

`npm run check` runs format/lint, foundation contracts and tooling coverage. Python tools are hash-pinned in `requirements-dev.txt`; JavaScript tools in the npm lockfile. Coverage is a minimum 90% statements/branches combined for the pure gate-policy modules (`gate_policy`, `review_policy`, `sonar_policy`, `check_app_coverage`, `rust_coverage`); wrapper/network/tooling execution is separately exercised by CI and negative-control tests. Do not call this whole-application coverage. Prototype source is a reference design, checked for syntax/assets/browser behavior; production lint, coverage and size budgets cannot exempt production code by moving it under design.

## Sonar ownership and operation

Use free, self-hostable SonarQube Community Build, pinned by image digest. Each CI job runs a loopback-only temporary server with disposable data, bootstraps random credentials, scans the candidate commit and validates the analysis before publishing evidence. No repository Sonar secret, external SonarCloud subscription or paid PR-analysis feature is required. This uses ordinary main-project analysis for each candidate; there is no persistent PR dashboard, trend history or paid PR decoration. The GitHub check and downloadable evidence are the review surface.

The gate verifies project identity and the analysis revision, populated metrics and quality conditions, no unresolved issues and no outstanding security hotspots. It also probes a nonexistent project to expose misleading empty API results. Coverage generation is a separate fail-closed job step; Sonar imports it rather than inventing coverage. Current scan scope is owned Python tooling and maintained JS test/configuration source; the non-production design specimen is excluded from this maintainability scan, not from secret scanning/browser checks. The application scan covers every maintained app/crate/package source and imports actual Rust/TS coverage and Clippy JSON. Generated OpenAPI/TS contracts have a separate drift gate. Sonar's Rust support imports Clippy and complexity/coverage metrics; it is not a comprehensive Rust security audit.

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

`python3 scripts/check_app_coverage.py --report REPORT.lcov --package OWNED_SOURCE_DIRECTORY`
validates TypeScript LCOV from the actual coverage generator. Invoke it separately for each
maintained package source directory, and add `--critical SOURCE_FILE` for each
money/inventory/authorization module. It requires every source in that directory
in the report, checks line/branch records against their totals and actual source
locations, and enforces 85% lines / 80% branches (95% / 95% for critical modules).
Missing branches fail even when line coverage is full. Generated code belongs in
a separate generated directory; exclusions require review rather than hidden
filename filters. CI must generate reports on the candidate revision before
calling this validator. Its hostile fixture tests prove the validator only;
application coverage remains pending until actual app/crate reports pass.

Rust coverage gates consume native LLVM JSON: stable line/region data and a
separate pinned-nightly branch run. LLVM LCOV summary line totals include async
and macro regions that do not map one-to-one to emitted DA records; do not rewrite
those totals to manufacture consistency. LCOV remains the Sonar import format.
The native validator requires every owned source and populated counters, with
85% lines / 80% instrumented branches per package and 95% for money, inventory
domain/stock/hold adapter modules and preview authorization. The match-only hold
SQL helper module has zero compiler-instrumented branches and requires 95% region
coverage. The staff guard, policy, operation wrappers and draft price adapter require
positive branch counters at the 95% critical line/branch thresholds. The draft
specification domain policy and guarded adapter have the same positive-counter
requirements; malformed stored JSON and logical corruption cannot bypass them.
Media policy, registry, rights and inspection modules also require positive
branch counters and 95% critical line/branch coverage. Tests distinguish rights
attestation validity from verified image bytes and publication acceptance.
Image-ingestion service/processor/storage modules require the same positive
critical branch counters. Tests use actual encoded synthetic bytes, orientation
and metadata fixtures, animation/truncation/header-bomb negatives, corrupt storage,
resource budgets, fresh authority, atomic complete sets and cancellation recovery.
Zero stays explicit rather than being reported as 100%. Existing critical
money/preview authorization still require positive branch counters. LLVM reports
zero instrumented branches for match-only and SQL adapter code; this is recorded
as `(0, 0)`, never 100%. Those packages additionally require >=95% region coverage.
Missing branch counters still fail. This compiler limitation remains explicit;
negative tests cover every locale arm and database failure/corruption outcome.

Keyboard assertions traverse each destination with keys and verify disabled
controls are skipped in both directions. A negative control removes a field from
Tab order and must fail at that field. macOS WebKit uses Apple's documented
[Option-Tab traversal](https://support.apple.com/en-gb/guide/safari/cpsh003/mac);
Linux CI uses Tab. This does not replace manual Safari/assistive-technology acceptance.

The P1 browser scope is the authenticated, noindex, read-only preview. The gallery,
locale/theme and fixture catalogue run on Chromium/Firefox/WebKit; Lighthouse
uses the production build for home/detail/gallery and k6 runs the isolated
catalogue fixture. Checkout/payment/account/room-planner acceptance stays pending
until those features exist. No passing dummy journeys are substituted. Initial visual candidates are captured
separately. `config/visual-approval.json` must record actual human approval and
bind every baseline image hash to the current UI source digest. Missing/stale
approval fails before comparison; snapshot updating is disabled in normal gates.
The CI browser artifact contains the candidate gallery for review. Local evidence
controls attest the contributor's record; they are not cryptographic proof of a
human review. Never create that record without the user's actual approval.
Baseline comparison uses Ubuntu 24.04 and the pinned Playwright browser builds.
Captures on other operating systems remain diagnostic and cannot replace the
approved images, since native fonts and controls can render differently.

Capture and comparison use native browser defaults at an explicit 1x pixel
density and the same viewport sizes. A correction limited to that comparison
environment can retain approval of identical images only when application UI and
assets are unchanged, all image hashes are preserved, the original approval and
source binding remain in provenance, independent review approves the correction,
and Linux CI compares every image successfully. Changed images or application
rendering require fresh human approval. Normal gates never update the binding.

Critical cases: last shared foam component contested by two orders; covers-only reserves no foam; payment-create timeout and late paid after expiry; duplicated/reordered webhooks; partial refund/tax/discount rounding; coupon exhaustion; expired guest token; cross-customer access; worker crash after provider acceptance; duplicate restock mail; incomplete translation/private-cache leak. Mutation testing on financial/authorization invariants must demonstrate that the test detects the intended defect, not just a textual mutation.

Translation history and review adapters retain the 95% critical line/branch gates.
Isolated PostgreSQL cases cover role separation, self-review, stale specification/
source/target revisions, full effect intent, competing editors/reviewers, expiry
through real lock waits, grant revocation, cancellation, immutable history,
malformed stored/source copy and schema upgrades without implicit review grants.
Native language quality and qualification are human acceptance evidence; fixtures
and passing tests cannot supply that evidence or make a locale indexable.

Catalogue definition policy and adapters have 95% critical line/branch requirements.
Isolated PostgreSQL checks exercise complete creation, canonical retries, competing
definitions/BOM edits, expiry through locks, revocation, cancellation, immutable
fixture identity, both directions of graph separation, partial commit rejection,
stored event binding, prior-reader migration compatibility and shared-component
stock conservation. Fixtures cannot become merchant stock or public catalogue
facts through the authoring path. Actual supplier and warehouse acceptance remain
P0 evidence.

Factual-review coverage includes exact-target inspection, each stale source,
historical withdrawal and non-reactivating receipts, all intent/actor fields,
review-only capability/confirmation, each independent source author, malformed
stored records, real lock-wait expiry/cancellation, revocation ordering, competing
reviews, raw-import history/independence/version constraints, stale repeatable-read
conflicts and upgrade from the previous schema. Domain and adapter sources retain
unchanged critical native line/branch thresholds; no zero-counter waiver applies.
Tests use invented merchant candidates only in isolated PostgreSQL databases.

## Accessibility and performance acceptance

Target WCAG 2.2 AA. Automated checks require zero serious/critical violations and triage all lower findings; manual review covers complete journeys with keyboard only, VoiceOver/Safari, NVDA/Firefox, 200/400% zoom, 320px reflow, high contrast, reduced motion and Arabic RTL. Check focus restoration, error summary, accessible names/live regions, contrast in every state and touch targets. Planner dragging has keyboard/input alternatives; 3D never hides the only dimensions. Native speakers review seven locales and fonts. Automated scores do not certify compliance.

Budgets are in `config/performance-budgets.json`: field p75 LCP <=2.5s, INP <=200ms, CLS <=0.1; lab median of three controlled runs Lighthouse >=90, LCP <=2.5s, TBT <=200ms; initial first-party JS <=170KiB gzip, mobile LCP image <=250KiB, fonts <=150KiB, total <=750KiB. Test home/product/cart/checkout/account and planner-open separately. Lazy-load room tools and nonessential motion; responsive images with dimensions, self-host subset fonts and cache-safe SSR. Never score the unoptimized specimen as production.

At 25 concurrent users for 10 minutes plus a 50-request burst, catalogue p95 <=300ms and quote p95 <=500ms excluding separately reported provider time, errors <=1%; no overselling, leaked resources or unbounded queue growth. Record hardware, DB size, seed, cache state and exact SHA. Resource caps and profiling matter more than language claims. P6 captures lab evidence; post-launch field data validates real-user performance separately.
