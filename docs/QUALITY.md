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

`npm run check` runs format/lint, foundation contracts and tooling coverage. Python tools are hash-pinned in `requirements-dev.txt`; JavaScript tools in the npm lockfile. Coverage is a minimum 90% statements/branches combined for the pure gate-policy modules (`gate_policy`, `review_policy`, `sonar_policy`, `check_app_coverage`, `rust_coverage`, `visual_policy`, `check_callback_edge_deploy`); wrapper/network/tooling execution is separately exercised by CI and negative-control tests. Do not call this whole-application coverage. Prototype source is a reference design, checked for syntax/assets/browser behavior; production lint, coverage and size budgets cannot exempt production code by moving it under design.

## Sonar ownership and operation

Use free, self-hostable SonarQube Community Build, pinned by image digest. Each CI job runs a loopback-only temporary server with disposable data, bootstraps random credentials, scans the candidate commit and validates the analysis before publishing evidence. No repository Sonar secret, external SonarCloud subscription or paid PR-analysis feature is required. This uses ordinary main-project analysis for each candidate; there is no persistent PR dashboard, trend history or paid PR decoration. The GitHub check and downloadable evidence are the review surface.

The gate verifies project identity and the analysis revision, populated metrics and quality conditions, no unresolved issues and no outstanding security hotspots. It also probes a nonexistent project to expose misleading empty API results. Coverage generation is a separate fail-closed job step; Sonar imports it rather than inventing coverage. Current scan scope is owned Python tooling and maintained JS test/configuration source; the non-production design specimen is excluded from this maintainability scan, not from secret scanning/browser checks. The application scan covers every maintained app/crate/package source and imports actual Rust/TS coverage and Clippy JSON. The contract gate byte-compares Rust OpenAPI, generated TS models and the generated openapi-fetch adapter; adapter inputs outside its supported operation subset fail closed. Compile-time controls cover schema and wire types. Sonar's Rust support imports Clippy and complexity/coverage metrics; it is not a comprehensive Rust security audit.

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
separate pinned-nightly branch run. The normal coverage command uses distinct
owned target directories for these toolchains, including cleanup and nightly
LCOV export. Sharing instrumented output can mix incompatible object mappings
and produce false zero-hit regions; never repair those counters by hand.
LLVM LCOV summary line totals include async
and macro regions that do not map one-to-one to emitted DA records; do not rewrite
those totals to manufacture consistency. LCOV remains the Sonar import format.
LLVM reports branch summaries from function instantiations and merges LCOV
branch details by source location. Those views can disagree when instantiations
cover different directions or a direction is folded. Validate LCOV summary
fields against the native summaries. Independently project the native file and
function branch details by location and require both to match every LCOV branch
identity and hit count. These details must account for at least the native
summary's total and covered directions. Native coverage thresholds use the
original summaries.
If the coverage step fails after exporting nightly reports, CI preserves the raw
JSON and LCOV under a separate revision-bound diagnostics artifact. Sonar only
consumes the application artifact published after successful validation and build.
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
catalogue fixture. The Application gate has a 65-minute job limit to accommodate
these checks and the full visual comparison. Checkout/payment/account/room-planner
acceptance stays pending until those features exist. No passing dummy journeys are
substituted. Initial visual candidates are captured separately. `config/visual-approval.json` must record actual human approval and
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

Image-suitability policy and adapter sources retain positive 95% critical native
line/branch counters. Real isolated PostgreSQL tests exercise current source/rights
checks, independent authors, historical withdrawal, full-intent retries, competing
reviews, permission/session expiry through real waits, authority revocation,
cancelled writes, immutable schema history, stale imports, private derivative
access, corrupt records and prior-schema upgrades without seeded approvals.
Synthetic encoded test bytes never establish actual photography or merchant
acceptance; no visual baseline or publication gate is replaced by these tests.

Publication domain/adapter sources retain positive 95% native critical line/branch
requirements. Isolated PostgreSQL cases cover complete approved selections,
missing/withdrawn/reapproved source decisions, Dutch-source dependency, each draft
change, exact integer prices, closed raw imports, immutable contiguous history,
stable path binding, full-intent retries and competing publishers. Real wait cases
cover licence/session expiry, cancellation, status/withdrawal ordering, revocation,
bounded failures and stale repeatable-read imports. Stored-row/source corruption
and corrupted original path bindings are rejected. Real eligibility-query and
deferred-constraint waits must finish before final session/licence evaluation.
Upgrade starts without publication or path records. No test
claims real supplier truth, qualified native review or public storefront acceptance.

## Accessibility and performance acceptance

Target WCAG 2.2 AA. Automated checks require zero serious/critical violations and triage all lower findings; manual review covers complete journeys with keyboard only, VoiceOver/Safari, NVDA/Firefox, 200/400% zoom, 320px reflow, high contrast, reduced motion and Arabic RTL. Check focus restoration, error summary, accessible names/live regions, contrast in every state and touch targets. Planner dragging has keyboard/input alternatives; 3D never hides the only dimensions. Native speakers review seven locales and fonts. Automated scores do not certify compliance.

Budgets are in `config/performance-budgets.json`: field p75 LCP <=2.5s, INP <=200ms, CLS <=0.1; lab median of three controlled runs Lighthouse >=90, LCP <=2.5s, TBT <=200ms; initial first-party JS <=170KiB gzip, mobile LCP image <=250KiB, fonts <=150KiB, total <=750KiB. Test home/product/cart/checkout/account and planner-open separately. Lazy-load room tools and nonessential motion; responsive images with dimensions, self-host subset fonts and cache-safe SSR. Never score the unoptimized specimen as production.

At 25 concurrent users for 10 minutes plus a 50-request burst, catalogue p95 <=300ms and quote p95 <=500ms excluding separately reported provider time, errors <=1%; no overselling, leaked resources or unbounded queue growth. Record hardware, DB size, seed, cache state and exact SHA. Resource caps and profiling matter more than language claims. P6 captures lab evidence; post-launch field data validates real-user performance separately.

Approved preview read/stock/image boundaries and HTTP mapping retain 95% critical
native line/branch gates. Isolated cases cover exact customer facts/price, missing
locale and fixture rejection, all-component stock/quarantine/reservation effects,
shared covers-only stock, pagination through withdrawals, source/review/rights
invalidation, final/post-decode clocks, withdrawal ordering, cancellation, corrupt
storage, redacted failures and authenticated no-store sanitized image responses.
These synthetic cases do not establish merchant truth or human visual acceptance.

P2 collection acceptance adds exact variant and locale navigation, served HTML/XML/
image agreement, genuine unavailable-SKU 404s and actual read-through stock,
draft-price, explicit publication and withdrawal effects. The test-only effect
example requires the per-run guard of the disposable browser database; its
credentials never enter evidence. TypeScript boundaries reject private/unknown
fields, malformed cursor/product identities and unbounded image streams. Money
formatting checks the maximum exact public integer without dividing a float.

The visual inventory covers collection, covers/filled details, empty EN/Kurmanji
collections, missing products, home, gallery and the room planner. The planner
adds a filled Dutch and Arabic case in each browser, theme and viewport for 252
expected images. The room-fit source and planner fixture are part of the digest.
Test a long planner conflict list with keyboard document scrolling at desktop
width. The semantic list is fully readable without a nested scroll area or an
extra tab stop.
UI changes
still require actual human approval bound to all image/source hashes. A failed
browser job captures a separate candidate gallery for review while the normal
required gate remains failed; captures never update baselines or grant approval.
If the visual check already completed the gallery, CI uploads that capture.
The fallback runs only after a browser failure without a completed capture.

Visual cases start with their declared native device theme before navigation,
then verify the explicit theme and inherited text colour after fonts and paint
frames settle. Capture and comparison use the same setup. Theme changes and
stored overrides remain separate interaction cases. This avoids a reproduced
macOS WebKit screenshot repaint defect after changing colour scheme in place;
computed text styles were correct while the captured inherited text was stale.
No unreadable candidate is acceptable evidence and normal comparisons still
require newly approved image/source hashes.

Merchant SKU cursor comparison and ordering use explicit PostgreSQL C collation,
matching the ASCII identifier contract checked by JavaScript. A real isolated
shifted-ICU regression puts active identifiers on either side of a page boundary;
withdrawing the comparison or ordering collation must fail that oracle. Deployment
locale must not make valid catalogue pages unavailable or lose a continuation.

Initial P3 quote cases use isolated synthetic policies, never actual carrier or
tax evidence. Independent cent oracles cover extended-line/half-cent/mixed-rate
VAT and shipping, integer ceilings and canonical SKU order. PostgreSQL cases
cover absent/ambiguous policy, exact postcode/publication matching, source/rights/
locale withdrawal, basket-wide component scarcity, quarantine/reservations,
measured parcel limits, dependency failure, actual stock lock waits and task
cancellation. Final clock controls reject policy/licence expiry during waits.
Quote domain and adapter modules require 95% critical line/branch coverage.
A compiled rounding mutant must fail the half-cent oracle. No browser checkout,
provider sandbox, order, discount, refund or operational acceptance is implied.

Private market policy adapters require 95% critical native line/branch coverage.
Isolated PostgreSQL cases cover capability separation and self-review, exact
source/review history, stale approvals and full-intent retries, withdrawal without
reactivation, competing writers, real lock-wait expiry/cancellation and immutable
raw-import constraints. Reviewed quote cases check exact destination, no older
draft fallback, independent cent totals, final expiry after physical stock waits
and quote/withdrawal serialization. Upgrade seeds no policies or market grants.
Synthetic evidence references do not establish actual destination readiness.

Promotion domain modules retain 95% critical native line/branch requirements.
Independent cent oracles cover largest-remainder ties, weighted/mixed-VAT lines,
percentage half cents, caps, unchanged shipping, exact integer ceilings, corrupt
snapshots and many conservation/refund partitions. Compiled mutants must expose
percentage rounding, SKU tie order and cumulative refund VAT defects. Real
PostgreSQL promoted quotes retain latest reviewed market selection and source/
component claims; actual waits cover promotion/market/rights expiry and cancellation.
Closed promotion terms remain trusted server data, never a customer money policy.
Indicative quotes do not establish atomic redemption limits or executed refunds.

Guest basket ownership, session store, Origin/CSRF, cookie normalization and wire
shape validation retain the same 95% critical native line/branch gates. Closed
object decoding lives with the HTTP contract types; no DTO branch exception is
required. Independent oracles cover two owners, wrong/duplicate/missing headers,
expired and revoked credentials, forged stored identity/receipts, exact historical
retries, version contention, quota/clock rewind and closed money/role fields.
Real isolated PostgreSQL lock waits prove revocation ordering, expiry before and
after effect writes, cancellation rollback and bounded issuance. Compiled final-
clock and CSRF mutants must fail their intended assertions with source restoration.
Trace log checks include a positive capture control and secret/body markers; the
log test runs in its own binary so concurrent subscriber interest caches cannot
produce a false zero. These HTTP/store tests do not establish a public browser
checkout, deployed cookie acceptance, provider payment or physical fulfilment.

Promotion policy and shared borrowed-clock code retain 95% critical native
line/branch gates. Isolated PostgreSQL cases cover separate capabilities, recent
MFA, self-review, canonical full-intent receipts, competing drafts/reviews,
withdrawal/new-draft fallback denial and lifetime limit snapshot preservation.
Actual waits cover source/withdrawal ordering, revocation, post-insert expiry,
backward time and cancellation rollback, with a valid write positive control.
Corrupt restored shapes/headers/limits, immutable raw imports, stale repeatable-read
writers and upgrade preservation are independent negative/positive controls.
Compiled removal of the observation wrapper, source SHARE claim and promotion
capability must fail the intended real-clock, lock-ordering and role assertions.
Configuration tests do not prove atomic redemption budgets, per-customer identity,
public coupons, provider sandbox or operational promotion acceptance.

For private owned promoted quotes, use literal mixed-rate discount and tax
fixtures. Exercise guest ownership, CSRF, ineligible or withdrawn sources,
expiry during a real source wait, old quote compatibility, shared issue limits
and cross-format retries. Reject damaged immutable source or amount records.
Check that the legacy projection rejects the promoted format and that indicative
issue and replay leave stock, orders and redemption usage untouched. For order
acceptance, race distinct baskets for the last redemption and the last available
cents under an actual anchor wait. Assert exact immutable usage across policy
revisions, rollback after late expiry or write failure, replay and ownership,
and native rejection of a missing or mismatched debit. Verify exact discounted
amounts in the private test payment, paid recognition and refund reservation,
with the original quantities in stock settlement. Keep the existing critical
coverage and isolated PostgreSQL gates.

For account-bound indicative quotes, use actual verified customer sessions.
Check same-account replay, a different account with the same basket, different
guest ownership, rotation or revocation, and expiry while a real write waits.
Assert the shared guest quote quota and effect namespace, immutable account
association and guest order rejection. Exercise a capped promotion as the
positive verified control.

For account-bound order acceptance, race two distinct baskets under the same
verified account for the last allowance. Admit a different verified account
while global capacity remains. Check counts across a new approval and denial
after anonymous usage, exact replay, expiry after a write wait, and rollback of
stock, order, debit and event. Exercise legacy guest read, payment and link denial
against an account-bound order.

For account-bound payment preparation, assert the literal discounted charge and
original stock group. Exercise guest and wrong-account read and replay denial,
session expiry after an actual outbox write wait, native account forgery and
legacy guest attempt replay across the migration.

For pre-attempt cancellation, race cancellation and first payment preparation
in isolated PostgreSQL and verify one effect survives. Check both guest and
account ownership, exact replay, prepared and started attempt refusal, native
event binding and rollback after expiry during a write wait. Verify that the
original stock and redemption remain available to their respective readers.

Whole-basket reservation logic retains 95% critical native line/branch thresholds.
Independent PostgreSQL oracles cover a last shared component, complete loser
rollback, changed/omitted/extra retry intent, immutable raw imports, legacy child
adoption denial, existing expiry compatibility and covers-only stock. Named
connection/lock instruments force opposing SKU/component orders; both whole
baskets must complete. Compiled omission of global component claims must fail
that oracle. Real waits prove final expiry, rewind and cancellation rollback;
scripted clocks also test admission-to-completion rewind and clock failures.
A caller-owned transaction test proves reservations remain invisible until the
outer commit and disappear after a later caller rollback. These primitives do
not prove customer checkout, provider sandbox or operational stock acceptance.

Owned quote snapshots and the shared observation clock retain the 95% critical
native line/branch gates. Independent integer oracles and corrupt stored objects
check exact source/header bindings, bounded timestamps, canonical lines and full
net/VAT/gross reconstruction. Isolated PostgreSQL tests cover exact concurrent
retries, old receipt/new cart separation, two owners, revoked credentials,
failed/successful issuance quotas and immutable imports. Actual post-insert waits
isolate policy expiry before quote expiry, photo-rights expiry before quote expiry,
quote deadline, backward clocks and cancellation rollback. Positive wait controls
establish the instrument admits a valid quote. HTTP tests assert private field
projection, closed authority input, exact CSRF/Origin, public locale mapping,
unready destinations and finite resource limits. Compiled omissions of the final retained-source check and owner SQL filter
must fail their intended policy-expiry and cross-owner assertions; source hashes
are restored afterward. Synthetic clocks and manually replayed cookies are
transport evidence, not native browser/device acceptance.

Private accepted-order tests use isolated PostgreSQL and explicitly invented
seller/contact/catalogue evidence. Literal captured cents, exact event payload,
shared-component balances and row counts verify one atomic order/hold/outbox effect.
Two shoppers contest the last components; concurrent receipt retries create one
order without extending stock. Current-source changes require new acceptance;
ownership, CSRF, revocation and changed intent reject access/effects. Real waits
after outbox insertion exercise final identity/market/photo/quote deadlines, seller
expiry, clock rewind and
cancellation rollback. Event-write failure rolls back all movements without private
database detail. Damaged-import controls exercise bounded object shapes, captured
money, header/intent/source mismatches and exact positive restoration. Domain
oracles cover explicit seller/tax/document/locale policy and bounded contact syntax.
Migration controls preserve prior quotes, stock and grants. Relational fault
instruments reject forged quote/intent/product/manifest bindings, and a missing
event fails the deferred completeness constraint. An alternate-effect cross-quote
sequence cannot reuse an acknowledged key for another order. Diagnostic controls
prove nested dependency errors lose private detail, with an unredacted positive
control. These source tests establish no actual seller, provider or fulfilment
acceptance.

The executable money mutation harness copies the actual Rust workspace manifests,
lockfile, crate sources and shared test fixtures into its disposable workspace.
Each invocation uses its own temporary build directory; no unmodified baseline
can inherit compiled mutants from a prior invocation.
It first compiles/runs the unmodified money/quote/promotion oracles offline with
locked dependencies, then requires each intended executable mutant to fail its
named assertion. A stale miniature dependency manifest, baseline compilation
failure or unrelated mutant compilation error fails the instrument; none counts
as a killed arithmetic defect.

Payment-attempt domain oracles use literal minor units and a literal stable key.
They cover positive dispatch boundaries, invalid clocks/configuration, unknown
outcomes after cache/hold expiry and credential rotation, immutable provider IDs,
exact amount/profile/metadata matching and preserved late-paid evidence. Closed
serialization rejects live mode, non-EUR currency, missing custody fields and
unknown statuses. Matching every supported status changes no order/stock state.
These pure tests establish no durable dispatch, provider or sandbox acceptance.

Frozen request tests assert a literal canonical JSON body and independently
computed SHA-256, exact decimal amounts through the integer ceiling, supported
method syntax, no recurring/profile/mode parameter injection, server-added return
identifiers, canonical HTTPS routes and configuration drift. Corrupt/oversized/
noncanonical stored bytes fail, with exact positive restoration. No such test
establishes method eligibility, actual callbacks or provider acceptance.

Durable payment custody uses real isolated PostgreSQL, including scarce stock fully
reserved by the accepted order, old quote expiry, later cart changes, exact amount
and request restoration, owner/CSRF/revocation negatives, changed effect/intent,
policy withdrawal and concurrent retries. Damaged imports must fail bounded closed
readers and remain tied to the original accepted order, even if a counterfeit
request and digest are individually coherent. Actual event-write waits exercise
cancellation, expiry, clock rewind and final authority rollback without extending
or replacing holds. Competing internal dispatchers obtain one initial permit and
one durable recovery event. Later calls reconcile after guest/policy/key/hold
changes; a lost or expired permit cannot authorize another initial POST. Delayed
permits also reject seller, market and photo-rights expiry before stock expires;
the durable start remains reconcilable. None of
these tests is provider sandbox, callback, deployment or fulfilment acceptance.

Compiled payment fault controls remove physical backing validation, remove the
owned-history query predicate, replace reconciliation with another initial
permit and bypass retained source deadlines. Each must compile and fail its specific custody assertion; the unmodified
baseline and exact restored source must both pass. Compilation errors cannot count
as killed defects. Source hashes and per-control logs bind the restored evidence.

The private Mollie transport uses a bounded loopback Hyper instrument and isolated
PostgreSQL, never actual provider resources. Literal decimal/fingerprint oracles,
critical object/mode/currency/metadata/sequence mismatches, profile preflight,
wrong credentials, redirect refusal, declared/streamed response limits, timeouts,
private-data redaction and safe optional links exercise both negative and positive
controls. Real held orders prove exact request bytes/idempotency key, separate
fetch verification, lost/malformed/rejected/mismatched create uncertainty without
another POST, retained source expiry and cancellation during gated provider I/O.
Independent transactions acquire attempt/stock UPDATE claims during that I/O,
proving the transport holds no database transaction. GET accepts genuine late-paid
state under a separately verified same-profile rotated test key; it changes no
order, stock, payment or fulfilment state. A wire response is not actual provider
sandbox or customer/payment acceptance.

Compiled provider controls bypass authenticated-profile verification, fetched
payment matching, the early streamed-byte limit, duplicate-field preservation and
sensitive authorization marking. Each must compile and fail its named assertion;
healthy baselines and exact restored-source reruns must pass. The streaming
instrument sends 65537 bytes then stalls, so a second JSON-size check after reading
the whole body cannot fake an early transport bound. These controls establish
source behavior, never actual Mollie acceptance.

Private unknown-ID list tests use literal fixed-endpoint/query assertions,
explicit one-page requests, separate required fetches and encoded opaque cursors.
Count/shape/ten-entry bounds, populated positional-array substitutions at all
critical nested objects, repeated identities/current cursors, hostile URLs,
duplicate/unknown parameters, original duplicate JSON fields, unrelated arbitrary
metadata and partial order/attempt matches exercise both positive and negative
controls. Rotated same-profile discovery is read-only; multiple exact identities
remain visible and empty/end pages grant no absence or create authority. Compiled
controls weaken the object-shape guard, provider-origin check, page limit,
near-match rule and raw entry preservation. Each must compile and fail its named assertion, with a healthy
baseline, exact source restoration and healthy rerun. These source tests do not
prove a complete durable scan, provider sandbox behavior or payment acceptance.

Verified-observation custody uses isolated PostgreSQL with real authenticated
loopback fetch results, immutable original orders and committed dispatches.
Exact/concurrent retries and distinct appends, overlapping claims under a
repeatable-read connection default, actual hold expiry, late paid,
reordered statuses/different provider IDs, key rotation, false original bindings,
clock/entropy/database failures and cancellation exercise fact/event atomicity
without stock or payment-state effects. Final clock rewind is tested after an
observed event INSERT wait; the blocker permits reads so the test reaches that
write. Native negative and healthy paired controls check deferred event
completeness, original outbox uniqueness, minimal snapshot binding, immutable
history, sequence/time bounds and exact event metadata. Damaged original dispatch identities/windows and damaged/oversized/orphaned
observations fail readers and retries; exact fixture repair restores a healthy read.
Populated positional arrays and every header have separate stored-decoder
controls. Financial/dispatch binding, connection isolation, event presence, operation-effect matching
and the post-write clock check require compiled fault controls with healthy
baselines, intended assertion failure and exact restoration. These tests provide
source custody evidence, not current payment state, provider acceptance or release.

Private discovery custody tests use authenticated loopback list results and real
isolated PostgreSQL. Literal multi-candidate/empty/end pages, actual hold expiry,
rotated credentials, receipt replay with unavailable ports, conflicting intent,
original-attempt drift, forks/cycles, the 100-page bound and concurrent same/distinct
operations exercise durable recovery without another POST or financial transition.
Overlapping claims under repeatable-read connection defaults must retain both
committed pages. Actual outbox-write waits test clock rewind and cancellation.
Native negative/positive controls exercise minimal snapshots, immutable history,
contiguous revisions/time, exact predecessor and deferred event completeness.
Closed decoder/header controls and damaged/oversized/missing-event imports reject
receipts and ancestor reads without fallback. Compiled controls remove exact cursor
binding, bypass complete original-attempt matching and retain a stale claim snapshot;
each must compile and fail its named assertion, with healthy baseline and exact
restored-source reruns. These instruments establish source custody behavior; supervised jobs and actual provider acceptance remain pending.

Private identity resolution tests use literal empty/single/multiple identity sets
and reordered paid/open facts. Real PostgreSQL verifies exact prefix receipts,
rotated fetched credentials, concurrent effects, historical replay without clock
or entropy, native false-summary/orphan/mutation controls and damaged source/event
reads. Independent source and SQL summaries must agree; zero observations never
proves absence and multiple identities never selects a winner. Bounded-prefix,
clock-after-write and paid-retention fault controls must detect the intended defect
and pass again with exactly restored source. Competing actual original orders
exercise same-profile ownership conflicts, sequential rebinding, actual stale
repeatable-read unique-claim waits, deferred claim completeness/immutability and
separate-profile positive controls.
These checks establish source evidence;
no current financial state, provider or fulfilment acceptance follows.

Supervised test-recovery checks use authenticated loopback GETs and real isolated
PostgreSQL. They assert complete candidate fetches, exact continuation checkpoints,
crash/page replay, stale-lease fencing, concurrent supervisors, no transaction over
provider I/O, sticky paid facts after a lost fetch checkpoint, bounded backoff,
poll/scan/step escalation and no automatic review restart. Unchanged later passes
must retain outstanding review/backoff counts and a nonzero command outcome;
separate-profile exceptions must remain isolated. Native source/page and
counter guards reject orphan/rebound jobs. Actual checkpoint-write waits exercise
expiry and cancellation; sampled-clock controls prove rollback after scheduling,
claims and progress writes. Literal malformed headers have paired healthy states.
Worker command controls reject production, live/missing/malformed credentials,
invalid/non-Unicode limits and diagnostic leaks before any provider operation.
Compiled fault controls must detect removal of lease fencing, receipt reuse and
final clock checks, then pass exact restoration. Source coverage and synthetic
GET responses do not establish actual provider, deployed or fulfilment acceptance.

Finance recovery checks use isolated PostgreSQL for exact concurrent replay,
conflicting intent, capability/session/revocation/MFA negatives, three lifetime
resumes and unchanged global budgets. An actual receipt-insert lock wait expires
MFA and proves both the audit and job update roll back. Native immutable/deferred
constraints reject orphan decisions, altered checkpoints, consumed-counter rewind
and repeated use of a historical decision after recreating its stopped state; schema 27 upgrades
preserve review stops and install no authority. A retained candidate checkpoint
survives exhausted authenticated GETs, resumes without another list/create and
retains late-paid evidence for separate resolution. Hostile stored-snapshot
controls reject open, missing and inconsistent objects. Compiled fault controls
must expose final-authority and lifetime-budget removal before exact restoration.

Private callback-ID fetch checks combine real isolated PostgreSQL with bounded,
authenticated loopback GETs. Literal wrong-profile/mode/currency/resource/ID,
metadata schema/identifier, sequence, nested-shape and duplicate-field controls
have paired healthy projections. Wrong original order/amount, unknown/unstarted
attempts and invalid effect/time/entropy/storage fail without effects. Late paid
and reordered fetches preserve sticky paid identity through separate resolution;
exact concurrent receipts append once and record rotated-key fingerprints. GET
cancellation retains no database claims or facts; encoded identifiers cannot
choose an origin/path/query and redirects cannot propagate credentials. Compiled
controls must expose removal of profile scope and original-attempt matching, then
pass exact source restoration. This is source evidence, not installed callback
handling or provider payment acceptance.

Durable private callback lookup tests use real isolated PostgreSQL and
authenticated loopback GETs. Server-generated operations commit before unique
single-use fetch starts; concurrent callers cannot issue a second GET for the same
operation. Saved receipts recover without clock, entropy or network access.
Cancelled/uncertain starts remain unresolved, while fresh operations retain
late-paid and reordered facts. Independent UPDATE claims during gated GETs prove
the transport holds no database transaction. Receipt/event write waits exercise
clock rewind and cancellation with complete rollback. A lookup preceding the
original dispatch cannot gain observation authority from a later dispatch.

Unknown originals retain dedicated callback custody without a checkout order or
order outbox event. Complete matching, namespaced observation replay and actual
stock expiry preserve the existing observation/identity semantics. Native
negative/healthy controls cover immutable operations, starts, receipts and events,
exact minimal projection bindings, deferred completeness and schema 28 upgrades.
Damaged bounded snapshots, headers, starts and events fail stored reads and
processing without falling back to another GET.
Immutable event digests must reject an otherwise-valid `open` to `paid` snapshot
alteration, as well as changed amounts or credential fingerprints, on restarted
reads and processing; exact restoration supplies the paired healthy control.
The new domain, transport and nested custody readers retain the 95% critical
line/branch gates. Compiled fault
controls must expose missing operation binding, event custody, snapshot digest,
dispatch chronology and final clock checks, then pass again after exact source
restoration. This is source custody evidence; bounded public ingress/supervision
and actual provider/deployment/payment
acceptance remain separate gates.

Private callback inbox and supervisor acceptance uses isolated PostgreSQL and
authenticated loopback GETs. Literal global/profile rate and capacity boundaries,
concurrent admission, full intake-event headers and final clock checks protect
atomic admission. Native active-custody and consumed-generation receipts reject
false settlement, source/profile rebinding, orphan jobs and historical lease reuse.
Capacity remains occupied while any older generation lacks a successful receipt
disposition, even after the current checkpoint completes.

Recovery exercises every crash boundary, a committed receipt behind a bounded
processing backlog, five lifetime GET generations, backoff and persistent review
stops. Expired workers may retain authenticated paid facts after newer open results
or terminal stops; they cannot advance checkpoints. Actual database waits prove
no transaction spans provider I/O and exercise lease expiry, clock rewind and
cancellation rollback. A saved receipt recovered under an earlier clock must leave
no earlier immutable observation/resolution and remain recoverable later.

Malformed lookup/event data and deterministic original identity-history failures
must retain source-bound redacted exceptions while unrelated healthy jobs progress.
An exception cannot be created before a raw receipt exists or grant payment
authority. Stored import controls have exact healthy repairs; unchanged later
worker passes retain outstanding failures. Crash-after-disposition controls damage
receipt, observation and resolution proof independently: each must preserve the
original disposition, quarantine its source and allow a healthy successor to
progress. Exact proof repair cannot silently erase that quarantine or release its
capacity. Worker configuration rejects production,
live credentials, malformed bounds and diagnostic leaks. New inbox/domain/worker
modules retain 95% critical coverage and relevant compiled fault controls. This is
source supervision evidence; public ingress, provider and operational acceptance
remain separate requirements.

The optional private classic-form API adapter requires the existing preview
Bearer gate and validated configured profile, with no provider key. Literal raw
body/header and decoded-ID bounds, malformed percent/UTF-8, duplicate encoded
names, unknown fields, media parameters and content encoding are exercised before
database effects. Isolated PostgreSQL proves atomic intake custody, native quota
and capacity limits, a deferred-COMMIT barrier before acknowledgement, timeout and
external cancellation rollback, and clock rewind after actual lock waits. Real
chunked transport exercises body limits; positive leak calibration checks captured
output as well as static errors. Configuration and callback modules require 95%
critical coverage. These checks cannot establish public staging/provider acceptance.

The disabled staging edge requires actual TCP-peer positive/negative controls,
including forged forwarding headers, and literal global concurrency/body/deadline
boundaries. Loopback transport fixtures exercise fixed targets, fresh header
selection, disabled proxies/redirects/POST retries, nonempty/truncated responses,
timeouts and cancellation. A real private API with isolated PostgreSQL proves the
deferred-COMMIT acknowledgement boundary across both services. Healthy binary tests
shut down normally so native profiles measure startup and server wiring. Config,
edge handler/binary and forwarding modules require 95% critical coverage.

The pure deployment-policy checker is included in tooling coverage. Semantic
positive and hostile mutation controls check the actual optional overlay's scope,
default-off routing, resources, buffering and absent secret labels. Compose merge
and interpolation checks use synthetic inputs without creating resources. These
checks cannot establish installed Traefik behavior, proxy-peer uniqueness,
network bypass, ingress upload timeouts, TLS or provider acceptance.

Private payment recognition requires literal independent state/money oracles and
real isolated PostgreSQL. Zero/open evidence remains unconfirmed; paid then open
retains received gross history; later distinct identities require review. Expired
holds do not erase genuine payment facts. Historical receipts and canonical
same-prefix replay survive later evidence, duplicate resolution receipts and
unavailable clock/entropy without another journal or event.

Actual competing transactions exercise observation append against recognition and
current inspection, including a pool whose default isolation is repeatable read.
Native SQL rejects non-READ-COMMITTED financial writes, stale prefixes, forged
state/money/order/resolution, missing source events/ownership, detached or rewound
pointers and incomplete journal/event/pointer commits. Captured historical reads
remain valid after a new observation makes the pointer explicitly stale. Bound,
malformed and noncontiguous evidence controls require paired healthy restoration.
Actual outbox waits test final clock rewind and cancellation with atomic rollback.
Tests compare accepted order snapshots and stock before/after. All recognition
sources retain 95% critical coverage; compiled fault controls exercise the financial
proof boundaries and pass after exact restoration. This establishes source custody,
not provider acceptance, stock recovery, refund or fulfilment authority.

Worker loopback tests execute the same configuration, profile verification,
supervisor and reporting code through a private client-factory seam; production
still selects the fixed provider client. Infrastructure's small build script
generates only a test module declaration for the canonical, fixed worker source
path. This compile-time wiring avoids duplicate lexical filenames in LLVM
coverage; it adds no runtime origin setting or commerce policy. Native reports
must contain one canonical worker source entry with actual counters. Do not merge
duplicate counters, rewrite them or exclude the worker to satisfy the gate. The
nonlogic build/test wiring is reviewed and covered by compilation, fmt and Clippy.
The worker logs a closed aggregate record and uses the same count snapshot for
its command outcome. A capturing subscriber verifies every event field and literal
count, with an unexpected-field probe proving the detector is active. That
calibration runs as exactly one selected test in a fresh child process so parallel
tests cannot change its tracing callsite cache; the parent verifies selection and
success, and the child retains native coverage instrumentation. This also
keeps macro-span instrumentation from obscuring evaluated logging inputs; native
coverage counters remain unmodified.

Original reservation settlement requires independent literal balance/movement
oracles, covers-only and distinct shared-component cases, and the full 20 × 64
source bound. Real isolated PostgreSQL tests exercise concurrent replay, competing
expiry in both winner orders, observation append, opposite member ordering, actual
outbox waits, cancellation, clock expiry/rewind and rollback after partial work.
Native negative/healthy pairs distinguish shaped ledger rows from actual physical
changes, current unconfirmed from received recognition, and complete original
custody from corruption. Migration from schema 31 preserves the existing paid
order and reservation without implicit consumption. Historical reads survive
legitimate later warehouse/financial changes without granting another effect.

Early constraint-flush tests must prove the transaction completion seal blocks
later affected physical and financial writes, survives savepoint release/session
unlock, and is removed with rollback that restores pending proof. Unrelated
same-transaction work remains possible; a manually acquired matching advisory
lock cannot skip native completeness. Oversized stored footprints fail before
digest evaluation, with a paired healthy restoration. Domain/adapter settlement
modules retain positive native 95% line/branch coverage and compiled fault
controls at these proof boundaries. These source tests establish neither runtime
stock consumption nor late-payment reallocation, shipment or provider acceptance.

Late stock allocation requires one canonical allocation across original and late
paths, with a populated schema-32 upgrade fixture whose original received history
remains valid after new financial ambiguity. Real isolated PostgreSQL must prove
all-active deadline, expired and mixed originals, other orders' reservations,
covers-only and distinct codes, genuine 20 × 64 pairs and 1,280 distinct physical
component codes. Compare literal stage balances and every required movement;
measure the closed encoded envelope against its two-MiB bound. Shaped ledger rows
without real balance changes must fail deferred proof, with an identical healthy
probe. Event guards and deferred parent proof need separate negative controls.

Released or consumed originals require durable review rather than fresh stock.
Scarcity leaves no fresh group, hold, claim or physical effect. An actual queue
interleaving must prove savepoint rollback releases component claims, reclamation
refreshes availability, and disappearing scarcity returns without a permanent
case. Later restock, changed evidence or failed clock/entropy cannot automatically
restart an immutable review. Captured-active history must survive legitimate later
expiry with coherent SHARE claims; sequence values are not commit chronology.

Force healthy original settlement to commit after a reader's captured absence
snapshot. It must not be misreported as orphan corruption; a genuine pre-existing
orphan must still fail, with healthy restoration. Compiled fault controls cover
current received authority, actual physical parity, successful completion seals,
final post-write time, bounded digest evaluation and the absence-snapshot race.
All new late-stock modules, nested allocation adapters and modified reservation
sources retain positive native 95% line/branch gates without counter rewriting.
These checks establish private source custody, not runtime stock, provider,
shipping, field performance or production acceptance.

### Authorized stock scarcity retry

Use isolated PostgreSQL to verify actual paid scarcity, explicit authority,
a still-scarce attempt, restock and successful retry with the original case
and movement custody preserved. Exercise exact replay, conflicting intent,
generation exhaustion, unsupported case reasons, a changed original state
and denied staff or stale MFA. Check raw SQL cannot admit a detached decision
or allocate against another case. Verify deferred reciprocity and rollback,
including an early constraint flush, transaction failure or lock wait.
Maintain the maximum-basket and component conservation checks from the
underlying late-stock path. Tests and private decisions supply no provider,
shipment or production acceptance evidence.

### Private refund reservations

Use real isolated PostgreSQL with an accepted immutable quote and authenticated
received evidence. Verify literal cumulative SKU and shipping gross/net/VAT
across multiple reservations, including the captured remainder. Race competing
requests for the final available amount and same-key requests for the same
intent. Exercise rejected changed intent, changed actor, stale or ambiguous
recognition, and staff expiry after waiting. Use native negative and healthy
paired controls for incorrect amounts, a detached event, incomplete custody and
attempted same-transaction evidence changes after early deferred evaluation.
Confirm the order's stock journal and all provider-side state stay outside
this transaction. Local tests provide source evidence for reservation custody.
The order refund allocator and reservation modules use the existing 95% native
line and branch thresholds for critical financial code.
At maximum history, validate 16 distinct recognition prefixes through 1,000
observations and the complete 20-line refund journal under the existing
two-second statement budget. Keep historical and current proof checks against
gaps, time regressions, changed snapshots and incorrect reciprocal events.
With an intact longest prefix, damage an earlier resolution's time and verify
that read-only refund history rejects it until the exact source is restored.

### Private refund execution proposals

Use isolated PostgreSQL with real reservation and recognition custody. Check
that the proposal retains the literal SKU and shipping amounts and the exact
payment identity and observation watermark. Exercise stale, unrelated and
incorrect source references, missing prepare authority, changed replay intent
and concurrent proposals for one reservation. Pair native rejection of detached
or incomplete events with a complete transaction, and check that the source
cannot change after early constraint evaluation. Verify that the operation does
not change reserved amounts, stock or payment state. Proposal modules use the
existing 95% native line and branch thresholds for critical financial code.

### Private paid-parent reversal totals

Exercise the private paid-parent aggregate GET through authenticated loopback
HTTP. Cover matching and mismatched original payment identity, profile, mode,
amount and metadata. Check exact decimal minor units, omitted versus explicit
zero, reported amounts above original gross, malformed money objects and
duplicate aggregate keys. Pair the loopback with isolated PostgreSQL. Check
current source before the permit, one start per effect, rotated credential
custody, start expiry, pending replay, historical result binding, and rollback
when the reciprocal event is suppressed or altered. Keep provider acceptance
separate from this evidence.

### Private password credential custody

Use isolated PostgreSQL with two verified accounts and actual Argon2id
verification. Exercise registration-token misuse, wrong account, wrong or
expired proof, replacement, simultaneous redemption and stale sibling proofs.
Prove one exact replacement survives races and a suppressed write or late
clock check rolls back both effects. Native negative and positive controls
exercise detached consumption, cross-account proof and revision jumps.
Measure proof freshness after an actual lock wait. Check that only the current
hash remains and that the stored challenge verifier differs from the bearer
and registration verifier. These checks establish private credential custody.

### Private customer sessions

Use isolated PostgreSQL with real verified accounts and current password
hashes. Check wrong or missing credentials, verifier domains, reset racing
login, stale sessions after reset, fixed expiry, logout and concurrent
rotation. Verify a real database wait cannot extend authority and failed
rotation leaves the prior bearer usable. Use direct SQL controls for detached
rotation, incorrect revision and mismatched successor. Apply the critical
95% native line and branch policy to the session adapter. Browser and mailbox
acceptance require separate evidence.

### Private guest-order links

Use isolated PostgreSQL with actual accepted guest orders and verified
customer sessions. Check wrong basket, order, CSRF, mailbox, proof purpose
and session revision, one-use replay and expiry after lock waits. Race two
valid proofs for one order and prove a single link commits while the losing
proof remains unspent. Use native negative and positive controls for detached
consumption and cross-account custody. Assert the original order is
unchanged. Test the private reader against wrong owners, stale sessions,
damaged reciprocal custody and same-time cursor ties. Inspect the projection
for exact captured totals and absence of buyer details. Assert that later
withdrawal and newer Dutch source copy preserve accepted titles, while altered
captured titles and missing, mismatched or rejected source reviews fail closed.
Browser and delivery acceptance remain open.

Test credential admission against isolated PostgreSQL from separate pools.
Assert exact independent target and global caps, domain-case normalization,
purpose separation, committed debits across later failures, and fresh window
selection after a real lock wait. Test native constraints, backwards or
unavailable time, stale key versions and inconsistent active configuration.
Inspect target rows for fixed-length digests and bounded rollover. Public
enumeration, timing and overload acceptance remain separate.

At the actual login entry point, compare denial and quota debits for an unknown
account and a wrong password. Make the credential lookup unavailable in an
isolated database and verify that exhausted admission still returns denial.
Check that admitted lookup failures retain their debit and correct credentials
still require working admission storage.

Test proof admission at its trusted entry points. Interrupt proof storage in
an isolated database and distinguish a saturated target from admitted lookup
failure. Verify shared consumption and enrolment capacity, independent password
issuance and replacement quotas, and successful rollover with the exact
unspent proof.
