# Development plan

Read [architecture](ARCHITECTURE.md), [product](PRODUCT.md), [quality](QUALITY.md), [security](SECURITY.md), [discovery](DISCOVERY.md) and [operations](OPERATIONS.md). These are implementation requirements, not stack comparisons. Production remains disabled until P7 evidence passes. Keep PRs small enough to review; finish one vertical slice before widening scope.

| Phase                        | Work                                                                                                                                                                                                                                                                                                                                                    | Exit evidence                                                                                                                                                                                                          |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P0 — inputs, in parallel     | Obtain actual SKU/BOM and packaging data, market readiness, brand/domain, provider profile and legal/tax inputs from PRODUCT                                                                                                                                                                                                                            | Explicit pending/ready matrix; no guessed facts enabling checkout                                                                                                                                                      |
| P1 — real skeleton           | Create Rust workspace, SQLx migrations, API/worker, isolated PostgreSQL test environment, typed configuration, health/readiness and OpenAPI TS generation. Build Next.js SSR route skeleton, tokens, locale/theme resolver and accessible component gallery. Activate application stage and all real CI gates in the same PR; use seeded local fixtures | Reproducible locked builds; fmt/clippy/audit/licences; TS/lint; real DB migration/contract tests; component/a11y/visual and served-HTML tests; Rust and TS coverage; no placeholder scripts or skipped required checks |
| P2 — catalogue and stock     | Product family/variants, BOM/components, prices/materials/media/translations, publish validation, stock ledger/holds/quarantine, admin catalogue/warehouse permissions                                                                                                                                                                                  | Hostile database concurrency cases, component conservation, translation/rights publication failures, RBAC and cache invalidation; catalogue HTML/feed agreement                                                        |
| P3 — buying and payments     | Server quote, tax/shipping matrix, discounts, guest basket/checkout, Mollie adapter, payment attempts, webhook fetch/verify, outbox/reconciler, orders/invoices/refunds                                                                                                                                                                                 | Sandbox success/failure/pending/timeout/duplicate/reordered/late payment, oversell and discount-limit tests; exact minor-unit/refund allocations; recovery without duplicate charges or fulfilment                     |
| P4 — customer and operations | Verified accounts, guest linking, MFA/admin roles, favourites, restock alerts, emails, inbox, fulfilment/returns, reviews, app/support feedback, redacted logs and bounded failed-job replay                                                                                                                                                            | Ownership/CSRF/abuse negatives, real test mailbox delivery, duplicate-alert prevention, full order-to-return browser flow, audit evidence and privacy deletion/export                                                  |
| P5 — design and room fit     | Translate courtyard direction into responsive production screens; 2D room planner with measured fixtures, keyboard controls, unit conversion/rotation/tolerance. Original help content and seven reviewed locales. Optional local photo overlay after fit works                                                                                         | Approved desktop/mobile/RTL/light/dark/reduced-motion screens; independently calculated fit cases; no upload without consent; clear approximation limits                                                               |
| P6 — acceptance              | Whole-catalogue served-HTML/feed/discovery checks, manual assistive technology, complete EU postcode/tax matrix, production-build lab/load profiling, threat review, restore/rollback exercise                                                                                                                                                          | Quality budgets on actual production build, browser/device evidence, reviewed translations/legal/product facts, no open Sonar/security issues; documented residual limitations                                         |
| P7 — launch                  | Separate staging and production, secrets provisioning, market/provider toggles, production smoke, verified emails, real fulfilment rehearsal and monitored release                                                                                                                                                                                      | Exact release revision plus all production gates passed; rollback works; owner approves live commerce activation; no claim of live acceptance from CI alone                                                            |

## P1 work packages

1. Create actual Rust workspace and SQLx migration runner; deterministic clock/IDs, typed minor-unit currency, config validation, structured error/log primitives, API/worker health. Start with a genuine catalogue read path backed by an isolated seeded test DB. Do not create toy crates just to make checks green.
2. Add explicit CI commands in QUALITY and update `config/delivery.json`, workflow and live rules together. Cargo `--locked`, frozen frontend install, container digests and dependency/licence inventory. Import actual Rust coverage/Clippy and TS coverage into Sonar; reject missing/empty reports. Public CodeQL remains additional JS/Python coverage, not Rust security proof.
3. Generate OpenAPI and client from Rust; fail on contract drift. Same-origin API routing with cache boundaries. Add health/dependency timeout tests and migrations from empty + prior schema.
4. Build storefront and admin shells with seven locale routes, device negotiation, stable explicit locale rendering, first-paint theme, RTL, semantic tokens and accessible primitives. Gallery covers focus/error/loading/disabled/empty states in both themes. Keep sample fixtures distinctly labelled and unavailable to public production publishing.
5. Add a full seeded catalogue read journey, raw HTML discovery checks, keyboard and axe tests, responsive visual baselines and production-build size budgets. Manual baseline approval is required for screenshot changes; do not regenerate to silence regressions.

P1 acceptance activates application CI; it does not activate payment or establish release readiness. Missing business inputs can remain pending while tests use clearly isolated fixtures.

## P2 work packages

1. Verify expiry supervision before operational stock use. Provider recovery
   remains P3; late payment must attempt explicit atomic reallocation. Prepacked
   stock still needs explicit kit conversion without counting contents twice.
2. Add reviewed translations, factual review/publication validation and activation
   of approved catalogue revisions, including safe public derivative delivery. Missing P0 facts and fixture data
   must fail publication. Apply the staff capability guard to catalogue mutations,
   retaining audit reasons and no exposed preview write route.
3. Connect approved catalogue/availability projections to HTML and feeds, with
   publication/price/stock invalidation tests and agreement on exact variant facts.
   Keep the courtyard UI baseline and draft/noindex preview boundaries intact.

The current component catalogue supports only separable fixture variants. Sealed
BOMs are immutable and contain 1–64 distinct cover/foam components, each requiring
1–10000 units. Rust reads only sealed revisions, ordered by physical component
code. Legacy preview rows remain valid without inventing missing BOM data.
Warehouse receiving, counting, quarantine/release and disposal now maintain
component balances and append-only movement history in one transaction. Effect
keys bind the full request, including actor/reason; exact retries return the
original receipt without reapplying stock. Atomic holds now snapshot a sealed
BOM, multiply component requirements by bounded variant units, and lock physical
balances in component-code order. Release, consumption and expiry are distinct,
idempotent transitions, with owner checks for release/consumption. Windows use
explicit clock inputs and a maximum 30-minute TTL. A deferred database constraint
rejects incomplete holds; sealed snapshots cannot change. Late consumption fails
without silently releasing or reallocating stock. Warehouse HTTP/authenticated
admin operations and production expiry supervision remain unimplemented. The
non-production worker's explicit `--expire-holds` command performs one bounded
pass using an injected clock; `HOLD_EXPIRY_BATCH` accepts 1–100 (default 25).
Concurrent workers skip locked holds. Each hold has its own transaction and
failed effects roll back to a savepoint while retaining the row claim. Failed
holds receive a static failure code, attempt count and a 60-second retry deadline;
other eligible holds can proceed. Resolution retains the failure audit record.
The default worker command remains a read-only probe. Supplier batch and prepacked procedures
still require P0 evidence before real warehouse use.

Server-side staff session/grant consumers now separate catalogue/BOM inspection
from warehouse mutations. They require an enabled account, matching capability,
unrevoked/unexpired session and recent MFA attestation, with locks held through
the operation and clock checks after lock waits. Warehouse changes require explicit confirmation; audit identity comes
from the session. Installation has no staff authority. Actual login/MFA/session
issuance and HTTP mutations remain P4 work; no preview token grants staff access.

Draft price history now uses the catalogue capability guard. Each immutable
revision records integer EUR minor units, a bounded evidence reference, reason
and session-derived actor. Explicit confirmation and the expected prior revision
are required. Effect keys bind the complete request, including actor; exact
retries return the original revision, while conflicting or stale edits fail.
All writers serialize on the variant; stale repeatable-read imports must retry.
Authority is checked again after lock waits before commit or receipt return.
No price history is seeded, activated, published or substituted into the preview.
Evidence references are records for review, not proof of factual approval.

Specification drafts now snapshot a sealed BOM and its exact physical component
membership/kinds. They retain millimetre measurements/tolerances, cover fibre
fractions (integer basis points totalling 10000), care references, foam density
in grams per cubic metre with firmness evidence, parcel dimensions/gross grams,
and assembly/supplier/traceability evidence references. Input bounds are sanity
limits, not carrier or product promises. Staff writes require catalogue authority,
confirmation and the expected prior revision; immutable audit/effect history and
post-wait clock checks match the price guard. Staff reads decode closed typed JSON
and revalidate every stored snapshot. Drafts have no approval/publication state,
no seeded measurements and no effect on the preview or stock. P0 verification,
media/translation review and public projections remain pending.

Media metadata registration and permission grant/revoke consumers now use the
catalogue staff guard, explicit confirmation, immutable reason/actor history and
intent-bound retries. One digest has one immutable classification, preventing a
known fixture/generated image from receiving product rights through an alias.
Grant scopes distinguish product photography from labelled inspiration; terms
explicitly represent perpetual permission or an expiry. Current rights are the
latest revision, never a fallback to an older grant. Rights inspection uses the
same freshly authorized server timestamp for session and licence evaluation.
Serialization locks, a deferred anchor relationship and MVCC writes protect
concurrent changes and stale imports. Captured dimensions/digests/provenance are
drafts; the safe private object key does not prove decoded bytes, actual permission
or SKU suitability. Original-byte/derivative verification and public delivery
remain P2 work; HTTP upload/login/MFA controls remain P4. No media is seeded.

Private image ingestion now verifies actual JPEG/PNG/WebP bytes against the
immutable registry digest, codec and raw pixel dimensions. It rejects animation,
malformed/oversized data and over-budget derivatives, normalizes orientation and
re-encodes bounded 320/640/960-edge variants without upscaling or copying private
metadata. There are at most two CPU jobs and two admitted ingestion workflows, including
storage waits, with no pending CPU queue. Decoding happens
outside database transactions; fresh authority is required before atomic storage
and private byte return. Originals remain private in a bounded PostgreSQL bytea
adapter; hashes, metadata bindings, complete sets and recipe version are protected
by constraints/immutable history. A byte receipt proves processing, not authentic
merchandising/permission or SKU suitability. Actual asset/colour/quality review,
translation and publication gates plus public serving/cache policy remain open.
Decoder allocation limits are best-effort in the library; owned byte/dimension/
output-buffer checks supplement them, and P7 still needs process resource caps.

## Next session prompt

> Read AGENTS.md and ROADMAP.md, then docs/IMPLEMENTATION.md and its linked requirements. Implement P2 catalogue/BOM and stock in small reviewed PRs to develop using the selected Rust/Axum/SQLx/PostgreSQL and Next.js stack. Model product families/variants, shared physical components, publication validation, stock ledger/holds/quarantine and catalogue/warehouse permissions. Keep production disabled and the approved courtyard design. P0 inputs remain pending; use isolated labelled fixtures and never invent publishable facts. Use only free, self-hostable dependencies with reviewed licences. Retain every P1 gate and add hostile PostgreSQL concurrency, component-conservation, publication/RBAC and cache-invalidation tests. Preserve shared work; use the independent review receipt and merge gate. Report code/CI, provider and deployment evidence separately.
