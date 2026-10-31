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
2. Connect validated publication eligibility to safe public derivative delivery.
   Missing P0 facts and fixture data must fail publication. Retain current-source,
   review/rights and authority checks, audited mutations and no exposed preview
   write route. Actual HTTP admin/session/upload controls remain P4 work.
3. Connect approved catalogue/availability projections to HTML and feeds, with
   publication/price/stock invalidation tests and agreement on exact variant facts.
   Keep the courtyard UI baseline and draft/noindex preview boundaries intact.

The current component catalogue supports separable fixtures and private merchant candidates. Sealed
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
immutable registry digest, codec and raw pixel dimensions. It validates WebP
container/chunk ranges and canvas/native dimensions before decoder construction.
It rejects animation,
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

Copy drafts now require explicit title, description, materials, care, safety and
assembly text in each locale. They reference an immutable specification and an
exact Dutch source version; previews cannot supply missing source copy. Versioned
approval/rejection records require a separate `catalogue.review` grant, a different
actor from the draft author, explicit confirmation, review evidence and a
qualification reference. Installation grants no authority or approval.
Variant serialization protects source/version checks across edits and reviews;
stale specification, source or target copy cannot receive a new approval.
Independent rejection can withdraw historical copy despite newer drafts. Exact
historical retries return their original receipt without changing the latest
decision. Reads revalidate stored copy, source content and references. All new
operations consume the existing recent-MFA/session guard and check authority at
completion, including private reads. Actual qualified native review, legal/UI copy
review, factual approval, image suitability, public activation and invalidation
remain required; an attestation reference alone cannot establish those facts.

Guarded catalogue authoring now creates merchant candidates and sealed BOM
revisions. Company identifiers cannot reuse the DEMO namespace. Immutable creation
events bind the exact entity/variant/BOM, supplier/source reference, actor and
reason, with explicit confirmation and fresh catalogue authority. BOM lines are
bounded, deduplicated and canonicalized for order-independent retries. Source
versions serialize on the same variant row used by other catalogue drafts.
Known fixture flags/identities cannot change and fixture/merchant families or
components cannot mix. Merchant definitions retain NULL legacy preview price/URL
fields; real prices and URLs must come from approved publication. Legacy copy is
fixture-only, preserving prior readers and the courtyard preview. Creation never
receives physical stock; warehouse authority and movements remain separate. No
merchant fact, image, copy or publication approval follows from definition entry.

Factual review now records independent attestations over an exact merchant SKU,
sealed BOM, specification and EUR draft price. Reviewer-only reads return the
validated identity/BOM audit events, specification measurements/materials and
price evidence, plus the latest decision for that target. A reviewer must differ
from the identity, BOM, specification and price authors. New approvals require
current versions of all three histories; rejection can withdraw historical
approvals. Confirmed, optimistic decisions carry session-derived actors, reasons,
evidence and qualification references; full-intent retries return old receipts
without changing later decisions. Database guards serialize raw imports, reject
stale snapshots and preserve immutable history. No approval is seeded and no
attestation activates products, URLs, stock, copy or public prices. Real supplier,
qualified reviewer, image and publication acceptance remain required.

Image suitability decisions bind a merchant SKU's exact BOM/specification, immutable
asset bytes and fixed processing recipe, and an exact permission revision.
Separate catalogue-review authority permits source inspection and sanitized private
derivative reads without granting authoring or original-byte access. New approvals
require current sources and active original product-photo rights, an independent
actor from identity/BOM/specification/registration/ingestion/rights authors, explicit
confirmation, evidence and qualification references. Variant and asset locks span
decisions; final session and licence clock checks reject expiry during waits.
Historical rejection remains possible after source or rights changes, and replay
never restores a later withdrawn approval. Immutable SQL history also serializes
direct imports. These attestations do not verify actual visual/material/colour
fidelity or qualified review and do not publish products or derivatives.

Explicit catalogue publication history now binds approved facts, verified-image
suitability and reviewed locale copy to exact immutable versions. Non-Dutch copy
also requires the exact current approved Dutch source. Catalogue managers confirm
activation/withdrawal with optimistic history, a full-intent effect key and audit
evidence/reason. Independent source reviewers remain separate from source authors.
Variant-before-asset locks, raw-import/MVCC guards and final session/licence clocks
span activation and stable family-path assignment. Public integer prices must fit
the exact JavaScript integer range; no rounding or draft-price fallback is allowed.
Private status reads revalidate complete source bundles and latest decisions under
locks. A historical activation can be ineligible after a source/review/rights
change; reapproval never silently reactivates it. Withdrawal keeps history and the
stable path, and historical retries return only their original receipt. Authenticated preview HTTP now reads a complete customer-field projection and
revision-bound sanitized derivatives from these records. Availability comes from
the sealed BOM and physical balances. Current-source, review and rights checks
run on each no-store request; image eligibility is checked again after decoding.
HTTP mutations and production remain disabled. Private collection/family HTML
and an explicitly labelled XML preview feed now consume the same validated
projection. Selected SKUs never fall back to another configuration; unavailable
products return genuine 404s before page streaming. Locale switching resolves the
selected SKU to its locale family path. Bounded no-store reads and revision-bound
images reflect stock, draft and explicit publication changes on subsequent
requests. Human visual approval of the expanded gallery remains required; actual
product and qualified-review evidence stays P0, and released merchant feeds/Offer
schema/indexability stay P6 acceptance work.

## P3 work packages

Staging for provider development uses a dedicated HTTPS origin and separate
containers/database/credentials. The noindex preview is private by default, with
an explicit staging-only public storefront read mode. A distinct `test-admin`
browser credential gates the unavailable admin shell; it grants no staff authority.
The API remains loopback-only inside the web network namespace; production mode
and live commerce remain rejected. The staging runbook owns resource caps, exact
source labels, read-only smoke checks and rollback. No fixture seeding, worker
supervision or live provider activation follows from deploying the empty preview.

1. Server quotes now read exact currently eligible catalogue publications under
   variant/asset claims and check aggregated basket demand against physical
   component balances. The closed request contains only destination and SKU/unit
   selections. Trusted server policies bind exact country/postcode, publication
   revisions, quantity limits, explicit item/shipping VAT and bounded parcel service.
   No policies are installed; missing or ambiguous readiness fails closed. These
   indicative reads neither reserve stock nor authorize payment. The initial
   VAT-inclusive arithmetic rounds VAT half up per extended SKU line and shipping,
   then sums allocations, with an exact JavaScript integer ceiling. Actual tax,
   carrier, territory and rounding acceptance remain P0/P6 requirements.
2. Private market policy drafts and independent exact-revision reviews now persist
   with separate market capabilities, immutable history and intent-bound retries.
   Reviewed quotes claim the current policy through catalogue/stock evaluation;
   newer drafts, withdrawal and expiry block old approvals. No policies/grants are
   installed. Complete the explicit country/postcode/territory readiness matrix
   from actual reviewed tax/carrier/returns inputs before enabling destinations.
   Promotion configuration now has immutable canonical drafts, independent
   exact-revision reviews, explicit EUR currency, separate unseeded capabilities and lifetime
   discount/redemption ceilings. Latest approved state only; configuration creates
   no coupon route or spending claim. Integrate atomic lifetime budget/redemption
   accounting with order transactions and verified customer limits. A guest cookie
   cannot establish a per-person allowance. Version 1 indicative fixed/percentage quote and captured
   refund arithmetic is implemented; it installs no promotion or refund authority.
   One promotion per quote, exact eligibility, UTC windows and immutable cent/tax
   allocations must remain consistent through checkout, payment and credit records.
   No customer coupon is accepted and no customer policy may grant readiness.
3. Durable guest intent now has a PostgreSQL-backed maintained session adapter,
   fixed Secure/HttpOnly host cookies, exact configured HTTPS Origin and CSRF
   checks, hashed credential custody, revocation serialization, bounded counters,
   optimistic versions and full-intent replay receipts. Internal routes are opt-in
   behind preview authentication; no frontend bridge or public activation exists.
   Owned indicative quote snapshots now bind basket revision, destination/locale,
   reviewed policy and publication/BOM revisions to checked allocations and a
   fixed maximum five-minute lifetime. Full-intent retries retain old receipts;
   final identity/source/deadline checks follow the actual insert wait. Complete
   trusted-edge abuse controls before a public customer bridge. Private immutable order/address/seller/market/product snapshots now compose
   whole-basket holds and a minimal outbox event in one transaction. Current basket,
   publications, reviewed policy and exact amounts are revalidated; changes require
   fresh acceptance. One quote creates one order and exact retries preserve history.
   Finish atomic coupon claims, lifecycle/payment transitions and retention before
   public checkout; seller/legal facts remain required trusted configuration.
   Expose the same-origin API/contracts and checkout UI only with those controls.
4. The private test-payment domain now models immutable attempt identity, exact
   EUR/profile/credential/request binding, one durable dispatch-start plan and
   strict server-fetched matching. It performs no persistence, network or payment
   transition. The application now freezes exact request bytes under validated
   explicit HTTPS return/callback configuration and matching configuration identity.
   Private owned-order attempts now persist exact captured amount/configuration/
   request bytes and their prepared event, with fresh ownership, current commercial
   eligibility and complete existing physically backed holds. Exact effect retries
   preserve historical receipts without new stock effects. One serialized dispatch
   start and reconciliation event commit before a single-use permit; later callers
   reconcile even after key/hold/source expiry. The private test-key HTTP client now
   verifies the authenticated expected profile, sends exact permit bytes once and
   separately fetches a bounded checked payment projection. It installs no runtime
   key, worker or payment transition. Bounded private unknown-ID list pages now
   return metadata-associated identity hints, preserving duplicate critical fields
   and validated fixed-origin cursors. Every candidate needs a separate checked
   GET; no/partial results cannot authorize another POST. Private verified-fetch
   observations now persist minimal exact original-attempt projections, actual
   fetched-key fingerprints and immutable operation receipts with processing
   events. They preserve reordered/different-ID evidence without selecting current
   payment state or affecting stock. Private authenticated discovery pages now
   retain complete original-attempt/requested-cursor/key bindings, immutable
   operation receipts and atomic processing events. Exact continuations reject
   forks/cycles and stop at 100 pages; empty/end/capped scans remain unresolved.
   Bounded readers revalidate every ancestor and event. Add identity resolution,
   supervised durable scheduling and worker processing before operational provider I/O;
   uncertain outcomes
   reconcile under the original identity and never mint a replacement charge.
   Fetch and verify webhook identifiers against order/amount/currency/profile/mode.
   Add durable outbox/reconciler, duplicate/reordered notifications and atomic late
   paid reallocation with visible recovery when components are unavailable.
5. Add separate order/payment/fulfilment/refund state, invoices/credit documents,
   bounded audited refunds and independent restock actions. Prove sandbox outcomes,
   concurrency, cent allocations and crash recovery without duplicate effects.

Trusted whole-basket reservation primitives now provide immutable manifests,
exact fresh child bindings and aggregated component claims in one transaction.
Historical retries preserve terminal members; expiry still uses the existing
worker. Integrate the caller-owned transaction entry with freshly authorized
checkout; the private order primitive now persists immutable snapshots, all
holds and an outbox event together. Add coupon claims and payment transitions. An indicative quote or standalone reservation grants no charge authority.

No public guest checkout route, provider calls, payment acceptance or production
activation follows from these private primitives. Preserve
all nine CI gates and the approved courtyard visual source/baselines.

## Next session prompt

> Read AGENTS.md and ROADMAP.md, then docs/IMPLEMENTATION.md and its linked requirements. Continue P3 in small independently reviewed PRs to develop. Complete the country/territory readiness matrix and promotions, then durable guest checkout, atomic basket holds and immutable orders. Keep missing P0 facts and unready destinations blocked. Mollie work is sandbox-only; preserve stable payment attempts, reconcile unknown outcomes, fetch/verify notifications and recover late paid stock explicitly. Retain exact money/refund allocations, all real CI/review/merge gates, the approved courtyard design and disabled production. Report source/CI, provider, deployment and fulfilment evidence separately.
