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

The API binary also supports `migration-manifest`, a configuration-free command
that reports its compiled migration metadata without opening a database. Compare
the actual image's output with independently collected SQLx records and retain
image identity; a matching manifest does not replace isolated startup and rollback
evidence.

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
   discount/redemption ceilings. Private guest order acceptance requires the
   latest approval and atomically appends to the promotion's lifetime usage.
   Verified promoted checkout binds a cumulative account debit to the held
   session and exact quote. An existing promotion with anonymous usage is
   unavailable under a later per-customer cap. Version 1 indicative fixed/percentage quote and captured
   refund arithmetic is implemented; an order-scoped allocator applies it to explicit
   SKU and shipping balances from accepted quotes. The private refund reservation
   command below derives the prior balances from immutable reservation history.
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
   final identity/source/deadline checks follow the actual insert wait.
   A private version-2 promoted snapshot additionally retains one exact
   approved promotion and its immutable original and discounted allocations.
   It shares the owner, effect namespace and quota and expires by the promotion
   end. Private verified quote issuance holds the current customer session and
   guest basket through commit and binds the quote to that account. These quotes
   can retain an approved per-customer term. Guest order acceptance binds the
   exact discounted quote and a cumulative immutable redemption to the new
   order. Account-bound acceptance requires the same current account and basket
   and retains the account's cumulative use across policy revisions. Complete
   trusted-edge abuse controls before a public customer bridge. Private immutable order/address/seller/market/product snapshots now compose
   whole-basket holds and a minimal outbox event in one transaction. Current basket,
   publications, reviewed policy and exact amounts are revalidated; changes require
   fresh acceptance. One quote creates one order and exact retries preserve history.
   An owned order can record a private cancellation before dispatch starts,
   including after a test attempt is admitted. The immutable receipt and event
   commit together. Cancellation and admission serialize on the owned basket;
   cancellation and dispatch serialize on the attempt and recheck durable state.
   Existing holds follow normal expiry and prior promotion usage stays consumed.
   Finish post-dispatch disposition, redemption disposition, lifecycle/payment
   transitions and retention before public checkout. Seller and legal facts
   remain required trusted configuration.
   Expose the same-origin API/contracts and checkout UI only with those controls.
4. The private test-payment domain now models immutable attempt identity, exact
   EUR/profile/credential/request binding, one durable dispatch-start plan and
   strict server-fetched matching. It performs no persistence, network or payment
   transition. The application now freezes exact request bytes under validated
   explicit HTTPS return/callback configuration and matching configuration identity.
   Private owned-order attempts now persist exact captured amount/configuration/
   request bytes and their prepared event, with fresh ownership, current commercial
   eligibility and complete existing physically backed holds. Exact effect retries
   preserve historical receipts without new stock effects. Account-bound attempts
   retain the exact verified order account. Private preparation and history require
   the same current customer session and guest basket. The native attempt guard
   checks that association against the accepted quote. One serialized dispatch
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
   Bounded readers revalidate every ancestor and event. Immutable identity-resolution
   receipts now bind complete fetched-observation prefixes, preserve distinct IDs
   as ambiguity and keep every paid fact under reordered statuses. No current
   payment or fulfilment state follows. Profile-scoped durable recovery jobs now
   provide bounded GET-only search/fetch/poll steps, receipt checkpoints, expiring
   fenced leases, backoff and irreversible operator-review stops through an explicit
   local/test worker command. No key or scheduler is installed. Private finance-authorized recovery now permits
   at most three explicit resumes of exhausted transient GET failures. Immutable
   before/after checkpoints, fresh session/MFA and full-intent receipt retries keep
   the original attempt, page, claim/poll/scan budgets and paid facts intact. Paid,
   ambiguous, capped and invalid-custody stops cannot use this action. Complete
   durable bounded callback ingress/queue processing, remaining operator dispositions
   and separate financial/late-stock transitions
   before operational payment acceptance.
   Private callback-ID GETs now capture only authenticated exact-profile Test/EUR
   evidence; metadata locates the original attempt but cannot establish its matching
   order/amount/dispatch. The complete original custody is revalidated before the
   existing atomic observation/event append. Durable private lookup operations now
   commit server-generated profile/provider identities and a single-use fetch
   start before a bound GET. Recovery reads the immutable minimal receipt first;
   started operations without receipts remain unresolved and cannot refetch. A separate callback
   aggregate/event pair retains authenticated unmatched facts without inventing an
   order. Closed stored revalidation and complete original matching protect the
   namespaced idempotent observation append. Complete bounded ingress, processing
   leases/budgets and visible review stops before a public callback exception.
   Private independent callback intake now applies atomic global/profile admission
   rates and unsettled capacity, with immutable source events and bounded jobs.
   Generation-to-lookup custody and one-use GET starts commit before I/O; receipt
   processing remains independent of current leases so old paid facts cannot be
   stranded after newer open results or review stops. Indexed unsettled custody
   releases capacity only after all generations have non-review dispositions.
   The explicit local/test supervisor retains five-generation budgets, leases,
   backoff and persistent redacted exceptions. Separate immutable quarantine
   retains damaged existing dispositions without blocking healthy successors or
   releasing admission capacity. A private Bearer-protected API adapter now accepts
   closed, bounded classic forms and acknowledges only durable admission. Complete
   runtime proxy and abuse-control verification before public ingress. An optional
   default-disabled Rust edge now gates actual connection peers and forwards
   bounded forms to the private API. Its optional exact staging POST router requires
   separate no-bypass, upload-timeout, TLS and provider acceptance;
   no key or scheduler is installed.
   Uncertain outcomes
   reconcile under the original identity and never mint a replacement charge.
   Fetch and verify webhook identifiers against order/amount/currency/profile/mode.
   Add durable outbox/reconciler, duplicate/reordered notifications and atomic late
   paid reallocation with visible recovery when components are unavailable.
5. Private gross-payment recognition now records canonical immutable decisions
   over complete authenticated evidence and their original resolution, with a
   separately validated current pointer and atomic digest-bound order event.
   New observations make earlier decisions explicitly stale; historical replay
   preserves paid facts without stock/refund/fulfilment effects. READ COMMITTED
   and the shared attempt claim protect new decisions and current inspection;
   detached receipts grant no later transition authority. Private original-stock
   settlement now atomically consumes the complete still-active reservation under
   that same current financial proof and attempt claim. One immutable receipt and
   digest-bound order event retain exact member/component effects. Native final
   proof and a transaction completion seal prevent partial effects or later
   same-transaction changes after early constraint evaluation. Historical replay
   revalidates original custody without repeating consumption. Private late
   allocation now shares one canonical allocation with original settlement and
   atomically expires/reserves/consumes the full frozen demand for intact expired
   originals. Released, consumed or scarce originals retain immutable review
   stops; changed availability after rollback creates no permanent scarcity case.
   A private recent-MFA `stock.review` inbox validates each bounded historical
   case before returning a minimal summary; no grant or HTTP route is seeded.
   Live keyset traversal requires a refresh to see earlier insertions. A separate
   private recent-MFA `stock.recover` command accepts an explicitly confirmed
   scarcity case revision and one bounded generation. It atomically records an
   allocation or fresh still-scarce assessment. Keep release/consumption cases
   stopped and require fresh confirmation for every new scarcity generation.
   A private recent-MFA `refund.prepare` command records a confirmed, bounded
   reservation against the original order's current received EUR recognition.
   It serializes the original attempt and cumulative SKU/shipping gross, net and
   VAT custody, with a reciprocal outbox event and historical exact replay.
   A separate private recent-MFA `refund.execution.prepare` command records one
   explicitly confirmed test-mode proposal per reservation. It retains the exact
   allocation, original profile-scoped payment and current received recognition
   with its observation watermark. Exact authorized retries return history. This
   proposal supplies no net balance or permission to contact the provider.
   Private test-key exact-item refund and chargeback GETs now require a matched
   paid original and retain minimal bounded observations only. A separate
   paid-parent test-profile GET commits its exact received source before I/O and
   retains four independent optional provider aggregates with a reciprocal
   event. Effect replay restores the operation or result. A read-only local/test
   worker command reports the exact stored operation's state and which optional
   totals are present, without contacting Mollie. A second read-only command
   validates and digests the bounded operation and receipt set under the attempt
   lock with current received recognition; later changes invalidate that snapshot.
   Net balance remains unresolved. One private,
   payment-specific test-profile list page now provides bounded reversal identity
   hints and a scoped continuation after verifying the same paid original.
   Every page retains the frozen attempt, payment, reversal kind, actual key
   fingerprint and requested cursor for receipt validation, including
   pages without candidates or a continuation. The private observation journal
   appends exact child facts after network I/O and
   serializes bounded custody with a reciprocal event. Same-effect retries
   verify the original observation. The page journal keeps a bounded linear scan
   and exact reciprocal events. A validated open tip restores a continuation
   under the same key. A rotated key starts a new root. The private page
   supervisor enrolls from an exact paid observation, commits a single-use
   start, and checks the matching page receipt before restoring a checkpoint.
   Expired starts without evidence go to review. Returned transport failures
   use bounded retries. Explicit local/test worker commands enroll, run one
   bounded pass and inspect a scan without exposing provider identifiers. For an
   uncertain stop, the inspection also validates the exact stopped operation and
   reports whether its late page is present. This detached result grants no
   retry or financial authority and cannot prove complete provider history.
   A separate item supervisor enrolls a verified page position and its paid
   original. It commits each operation before fetching the exact child and
   checkpoints only against the matching authenticated journal receipt. A private,
   unseeded recent-MFA `reversal.review` capability can inspect one exact item
   review stop, its bounded operation history and any exact-effect late receipt
   under the same staff and attempt claims. A private one-candidate inbox uses
   the same authority and full proof, with a live keyset cursor and no provider
   or state effects. A separate local/test worker review command accepts
   exactly 32 raw staff-session bytes on non-terminal stdin with a five-second
   EOF deadline, then reports only the closed review summary. Credentials do not
   appear in arguments or logs. Staff session issuance remains unimplemented;
   this command does not establish production operator access. Neither review
   path grants retry or financial authority. Add operator review recovery and
   scheduling before
   provider acceptance. An exact
   GET, 404, empty page, ended scan or capped scan
   leaves the provider history and net balance unresolved. Complete provider-side
   refund/chargeback reconciliation, execution custody
   and reservation disposition before moving funds. Complete independent
   order/fulfilment/refund lifecycle and invoices/credit
   documents, bounded audited refunds and independent restock actions. Prove
   sandbox outcomes, concurrency, cent allocations and crash recovery without
   duplicate effects.

A pure whole-variant shipment quantity check compares an accepted order's
captured SKU quantities with complete caller-supplied prior counts and a bounded
proposal. Before dispatch, persist authenticated fulfilment history and record
physical package evidence for the components in each shipment.

Trusted whole-basket reservation primitives now provide immutable manifests,
exact fresh child bindings and aggregated component claims in one transaction.
Historical retries preserve terminal members; expiry still uses the existing
worker. Integrate the caller-owned transaction entry with freshly authorized
checkout; the private order primitive now persists immutable snapshots, all
holds and an outbox event together. Add coupon claims and payment transitions. An indicative quote or standalone reservation grants no charge authority.

No public guest checkout route, provider calls, payment acceptance or production
activation follows from these private primitives. Preserve
all nine CI gates and the approved courtyard visual source/baselines.

## P4 private favourites

A verified customer can privately retain up to 25 merchant SKUs. Each operation
holds live account and session claims and serializes on an account-specific
bucket. Mutations require the current bucket revision unless already satisfied;
actual membership changes advance it. Adds require the current eligible
publication in the selected locale. Listing rechecks that projection and returns
only the current SKU and publication revision. Unavailable entries remain stored
but are hidden; removal and clear include them. A separately enabled test-only
GET uses preview authentication, the exact test Origin and the issued customer
cookie to read this projection. It can initialize the empty bucket on first read;
its decimal revision avoids JavaScript integer precision loss. It does not provide export, account deletion, browser
controls or production abuse and retention policy. Migration 0063 follows the
customer-session CSRF migration 0062.

## P4 private restock opt-in custody

An explicit private subscription retains one current merchant SKU per verified
account, independent from favourites and marketing. A 25-entry engineering cap,
serialized account revision and unique activation generation bound concurrent
changes and stale replays. New opt-ins require current publication in the chosen
locale but do not require a particular stock state. A customer can unsubscribe
an unpublished SKU or clear all active entries. A separately enabled test-only
GET uses preview authentication, the configured Origin and the customer cookie
to return current published SKU identities and a decimal bucket revision. It
may initialize an empty bucket; it does not change subscriptions, move stock,
create an order event or send a message. No public account route is installed.
Current opt-in time
is retained only while subscribed; this is not consent history. Alert scheduling,
delivery idempotency and cancellation ordering, retention, locale, recurrence,
privacy export/deletion, guest enrollment and abuse controls remain P4 work.
Migration 0065 follows the separately queued 0062–0064 migrations.

## P4 email verification foundation

Private SQLx custody issues a random challenge for a narrowly normalized mailbox
and consumes it once under purpose, identity and fresh-clock checks. Its fixed
15-minute expiry is checked after lock waits and again before commit. The direct
issuer returns its bearer to the trusted caller. The delivery issuer commits a
nonce reservation before encryption, then commits the challenge and its encrypted
envelope together. Its receipt omits the bearer. The schema and scope are
specified in [security](SECURITY.md#private-email-verification-challenge-custody).
A private start claim locks the live challenge, checks its exact prior envelope,
nonce and configured key fingerprint, then durably records one start. Repeated
claims while the source is live can observe that start without creating a second
one. The separate in-process opening path shares the exact claim transaction.
Only its new committed start can open the captured envelope. It re-locks and
checks the exact source, authenticates the complete ciphertext and verifies the
recovered bearer, then completes the transaction before calling the trusted
in-process consumer. A failed or interrupted opening does not restore the start.
An optional trusted synthetic callback records one bounded accepted, explicit
rejection or uncertain result against the exact earlier start, after releasing all
database locks. A start without a recorded result remains unknown, and the
envelope cannot be reopened. Results remain historical after expiry or consumption.
The database records the trusted caller's classification. Provider authentication,
mailbox receipt and redemption each require their own evidence. A provider,
retry policy and reconciliation remain separately reviewed work.
A private account operation now
locks and consumes that exact proof and inserts one unique immutable normalized
identity in the same transaction; the database binds consumption and insertion to
the same transaction, with legacy consumed proofs ineligible. Rust verifies bearer
possession; the database write role remains trusted. Replay and duplicate identities deny without
consuming another proof. A final fresh-clock check follows all writes and waits.
The identifier grants no login, session, guest-order or other authority. Production
delivery operations, key operation, mailbox acceptance and anti-enumeration remain required. Migrations 0034
and 0035 must land after the separately reserved 0033 late-stock migration; no
deployed migration state is implied.

Private password setup and recovery use a separate one-use proof tied to the
verified identity and current credential revision. Hash the proposed password
outside the transaction, then lock the account and recheck the exact proof.
Consume the challenge and replace the current Argon2id hash atomically. Flush
the deferred completeness check before the final clock check. Older challenges
cannot overwrite a newer credential. Private login verifies the password outside
the transaction and claims the account before rechecking the exact hash and
revision. Session claims require the same current credential revision. Rotation
consumes the old bearer and issues one replacement within the original 12-hour
window. Logout revokes the current bearer. An optional private login variant
returns an independent synchronizer once and commits its immutable digest with
the exact session. Protected logout checks both under the session claim; legacy
bearer-only callers remain compatible and rotated successors are unbound.
The separately enabled test-only HTTP and Origin/CSRF transport is described below;
public browser ingress and production CSRF policy remain open. The private login entry point commits
its purpose and target quota debit before lookup or hashing and carries the same
observed clock through issuance. Verification and password proof operations also
commit admission before accessing proof or credential state. Standalone proof
consumption and verified account establishment share one target budget, with one
debit per request. Private replacement additionally requires a trusted, immutable,
bounded and integrity-checked offline password blocklist snapshot; exact NFC
matches are denied after proof precheck and before Argon2. Snapshot validity is
rechecked before commit. No corpus is supplied. Obtain and review a real corpus
and coordinate its delivery, withdrawal and freshness. A loopback,
preview-authenticated test-only POST consumer can establish an account
from a supplied proof with one admission debit; it is disabled unless its test
origin and dedicated key are both configured. It issues no session or proof.
An independent default-off test-only login POST requires preview authentication,
an exact HTTPS Origin and a separate admission key. It performs the admitted
password login and returns the resulting session only in a secure host-only,
HttpOnly SameSite=Lax cookie with the original session expiry. Its empty response
contains no identity and its bearer-only session cannot use protected logout.
An independently enabled test-only login returns a bound synchronizer once in a
no-store response and the session in the same kind of cookie. Its paired logout
requires the exact Origin, one cookie and one matching proof. Only committed
revocation returns a removal cookie; a lost response can leave revocation committed,
and a duplicate request is denied. No session-read or public route is installed.
Complete secure delivery, trusted ingress, timing and browser controls before
exposing the flow beyond this test boundary.

Private guest-order linking requires a live guest basket credential and CSRF
proof to issue a one-use challenge for that exact accepted order. A current
customer session for the verified buyer mailbox consumes it and records the
immutable link in one transaction. Private detail and history reads hold
the live session claim while validating the reciprocal link and the original
order. Return captured items and amounts through a minimal projection.
Cookie and Origin checks, safe proof delivery, rate limits and public order
journeys remain separate work.

## P5 room-fit foundation

The standalone browser-safe room-fit package implements the measured rectangle
contract in [product](PRODUCT.md#room-fit-calculation-boundary). Exact integer
units, bounded input, quarter-turns, conservative tolerance, overlap and
user-supplied door clearance yield invalid, uncertain, conflict or fit results.
Independently calculated fixtures cover boundaries. The non-selling preview page
uses a keyboard form, local drawing and text status. Clearing the draft allows a
unit change without reinterpreting existing measurements. Use real approved
measurements for any future catalogue integration. A standalone versioned
codec can round-trip bounded editable drafts for explicit local files. The
preview validates a selected file before asking the user to replace the current
draft. It generates fresh form identifiers and clears the old assessment.
Downloads use a short-lived local object URL. Photo placement, original help
content and reviewed locales remain P5 work. Owner approval binds the 252
Linux visual baselines, including the file controls, to their exact preview
source. Hosted comparison and native browser and device acceptance require
separate evidence.

## Next session prompt

> Read AGENTS.md and ROADMAP.md, then docs/IMPLEMENTATION.md and its linked requirements. Continue P3 in small independently reviewed PRs to develop. Complete the country/territory readiness matrix and promotions, then durable guest checkout, atomic basket holds and immutable orders. Keep missing P0 facts and unready destinations blocked. Mollie work is sandbox-only; preserve stable payment attempts, reconcile unknown outcomes, fetch/verify notifications and recover late paid stock explicitly. Retain exact money/refund allocations, all real CI/review/merge gates, the approved courtyard design and disabled production. Report source/CI, provider, deployment and fulfilment evidence separately.
