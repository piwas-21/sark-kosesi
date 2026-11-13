# Implementation status

**Current stage: P3 checkout/payment custody in progress; production disabled.**

The read-only Rust/SQLx catalogue, migrations, typed contracts, API/worker readiness
and courtyard Next.js shells are implemented. Explicit clock/identifier ports
support deterministic diagnostics and generated request correlation. Real
source/coverage/audit, PostgreSQL, component/browser/accessibility, approved visual,
performance and discovery gates are required from the first application PR.
Current-revision CI, independent review and the merge gate remain mandatory.
The API's offline migration-manifest command reports the compiled SQLx migration
versions, types, checksums and missing-version policy. It does not prove image
provenance, database state or startup compatibility.

Product families, separable variants, shared cover/foam components and sealed,
immutable BOM revisions are implemented behind the existing read-only preview.
The database rejects fill mismatches and serializes sealing against line edits.
Existing P1 fixtures receive no invented BOM; prepacked stock remains unavailable.
Physical component stock now has append-only movements, receiving/counting,
quarantine inspection/release/disposal, reasons and actor records. The trusted
Rust primitive serializes component changes and rejects conflicting effect-key
retries; no warehouse HTTP operation is exposed.
Atomic holds now reserve a sealed BOM snapshot in physical-component order.
Release, consumption and expiry are distinct, owned, idempotent transitions;
covers-only never consumes foam. Clock inputs are explicit, with a maximum
30-minute window. Whole-basket trusted reservations now bind a complete immutable
manifest to fresh child holds, claim all shared components before effects and
commit every line atomically. The caller-owned transaction entry is ready for
future order/outbox integration; no customer hold/checkout route is exposed.
The worker now has an explicit non-production expiry command:
one pass claims at most 1–100 due holds with `SKIP LOCKED`. Failed effects roll
back, retain redacted retry state and retry after 60 seconds. The default probe
remains read-only. No production scheduler or payment recovery is enabled.

Catalogue staff can now record immutable draft price revisions with explicit
confirmation, an evidence reference, a reason and session-derived audit actor.
Optimistic revision checks and effect-key replay prevent lost edits and duplicate
history. Draft records do not establish approved merchant facts. Public preview
prices change only through the explicit reviewed publication flow described below.

Immutable specification drafts now record BOM-linked component measurements,
cover fibre fractions, foam evidence, assembly footprint, measured parcels and
traceability references. Staff reads decode and revalidate stored drafts; no
measurement/evidence entry grants factual approval or publication.

Media metadata now has an immutable content identity and audited rights grant/
revoke history. Known fixture/generated digests cannot be relabelled under a new asset ID
or receive product-photo rights. Rights consumers check explicit scope, terms,
revocation and fresh server time; positive rights alone do not verify image bytes
or product suitability. The approved publication path adds those checks below.

Actual original bytes can now be verified against registered digest/codec/raw
pixel dimensions. Bounded decoding normalizes orientation and produces private
privacy-metadata-free derivatives without upscaling. Fresh authority protects an
atomic, immutable PostgreSQL original/rendition set. Decoding alone grants no
public delivery or product/permission/SKU approval.

Catalogue copy now has immutable drafts tied to specification revisions and, for
non-Dutch locales, an exact Dutch source revision. A separate catalogue-review
grant and a different account are required to record approval/rejection with
review and qualification references. Source changes block stale new approvals;
independent rejection can withdraw a historical approval after newer drafts exist.
Exact historical retries do not reactivate old decisions. These records do not
provide actual native-reader acceptance or activate public copy.

Catalogue staff can now create unpublished merchant family/component/variant
definitions and sealed BOM revisions with immutable source/actor audit events.
Known fixture identity cannot be promoted; fixture and merchant component graphs
cannot mix. Candidate definitions receive no legacy price, URL or stock. Existing
preview data stays separate and shared physical stock remains component-based.

Independent factual review now binds exact merchant identity/BOM, specification
and draft price revisions. Review-only staff inspect revalidated source snapshots;
a separate actor from each source author records confirmed decisions with evidence
and qualification references. New approvals require all current source revisions;
historical rejection remains available, and receipt retries cannot reactivate a
withdrawn decision. These attestations neither verify actual supplier evidence nor
activate public product facts.

Image suitability review now binds exact merchant identity/BOM, specification,
verified private image recipe and rights revisions. Review-only staff can inspect
sanitized private derivatives; source authors cannot review their own work.
Approvals require current sources and active product-photo rights, including after
lock waits. Historical withdrawal and receipt replay preserve later decisions.
These records do not establish actual photographic fidelity or product acceptance.

Catalogue managers can now record explicit activation/withdrawal history over
exact approved fact, image and locale-copy decisions, including the Dutch source
approval for translations. Current source revisions and fresh product-photo rights
are required. Stable family paths are assigned atomically; source changes or later
review decisions make old activations ineligible without falling back or silently
reactivating them. Historical receipts never change the latest publication.
Authenticated non-production HTTP reads now expose a current approved customer
projection and exact revision-bound sanitized images. Physical BOM availability
is read through with no shared cache. No actual merchant facts or publication
records are seeded. Private collection/family HTML and a labelled XML preview
feed now consume the same projection, with exact SKU links, complete customer
facts and read-through stock/publication invalidation. The owner approved all
228 Linux screenshot baselines for the expanded preview; the approval record
binds the exact UI/fixture source digest and every image hash.

HTTPS staging supports public storefront reads by explicit configuration while
admin/unknown/mutation routes retain a separate test-admin browser gate. The API
remains loopback-only and authenticated, with a separate database and disabled production. Deployment/provider acceptance is
recorded independently from source/CI. Server quote source now reads approved publications and aggregated shared-component
stock with explicit exact-destination policies and bounded integer tax/shipping
allocations. No policies are seeded or accepted from clients. Quotes are indicative:
no stock is reserved and no payment is authorized. Private market policy drafts and independent approvals now persist exact country/
postcode, publication, VAT, parcel limits and validity windows with immutable audit
history. Separate market author/reviewer grants and recent MFA protect changes.
The latest draft and its latest review govern indicative quotes under one database
transaction; a newer draft or withdrawal blocks old approvals. No policies or
grants are seeded. Trusted indicative promotion quotes now retain original prices,
allocate capped fixed/percentage discounts by largest remainder, and split VAT on
discounted extended lines under the same final source/expiry checks. Pure cumulative
refund allocations preserve captured tax across partial credits. A pure order-scoped
allocator now checks explicit SKU/shipping balances and returns exact captured
gross/net/VAT deltas. A private, confirmed finance reservation claims these
amounts once against the accepted order and current received evidence, with
immutable cumulative accounting and a minimal outbox event. Private test-key
exact-item refund and chargeback GETs now verify the paid original and return
minimal bounded observations. A separate paid-parent GET records an immutable
operation against the exact received recognition and retains the authenticated
aggregate amounts with a reciprocal event. Local/test worker inspection can read one
exact operation or validate and digest the bounded current stored set without
provider access. Detached digests grant no financial authority; net balance remains unresolved. Payment-specific
test-profile list pages now yield
only bounded reversal identity hints and scoped continuations after verifying the
paid original. Every verified page retains the authenticated request scope,
including empty and terminal pages. A private journal records exact authenticated child
observations and their reciprocal events, with bounded history and effect-key
replay. It preserves amounts above the original payment and unresolved reversal
status. The private page journal keeps exact candidate order and cursor lineage
with effect-key replay. A validated open tip can restore a same-key continuation.
An explicit test-profile supervisor now admits bounded page operations from an
authenticated paid observation. Durable starts, exact receipt recovery and fenced
checkpoints keep uncertain work visible for review. Explicit local/test worker
commands enroll, run a bounded pass and inspect scan state, including the
validated presence or absence of the exact late page for an uncertain stop. A separate item
supervisor binds one candidate position to an exact authenticated refund or
chargeback observation. Private finance staff can now prepare one immutable
test-mode execution proposal per reservation. It preserves the reserved SKU and
shipping amounts and binds the current received recognition, observation watermark
and original payment under its frozen test profile. Exact retries return the
historical proposal under current staff authority. Operator recovery, provider
reversal reconciliation, reservation disposition and dispatch custody remain open.
Private promotion drafts and independent exact-revision reviews now persist
canonical eligibility/discount terms and explicit lifetime budget/redemption limits.
New drafts and withdrawal block older approvals for new orders. Private guest
orders now commit reviewed discount allocations, lifetime redemption usage,
whole-basket holds and a minimal outbox event together. Exact retries preserve
history and prior redemptions carry across policy revisions. Complete country and
territory readiness, cancellation disposition, public checkout controls,
order/payment lifecycle, Mollie sandbox attempts/
reconciliation, invoices and executed refunds remain P3 work under the [implementation plan](docs/IMPLEMENTATION.md). P3 owns
late-payment reallocation. Supervision of expiry remains a deployment gate.
P0 inputs remain pending. All seven languages are draft previews,
fixtures are explicitly not for sale, and admin operations remain unavailable.
Server-side capability guards now protect catalogue/BOM inspection and warehouse
commands using persisted staff sessions, recent MFA attestations and grants.
Warehouse changes require explicit confirmation. Revocation locks span the stock
commit; audit actors come from the session.
Installation seeds no staff authority. Login/MFA verification, session issuance
and HTTP admin writes remain unimplemented. Catalogue publication already uses
the same persisted authority guard.
Do not restart stack comparisons or redesign the accepted courtyard direction.

Private P4 email-verification challenge custody now provides exact normalized
identity binding, random bearer verifiers and single-use expiry under database
locking. A private account primitive now consumes the exact challenge and inserts a unique
immutable verified identity atomically. A separate revision-bound password proof
now supports initial setup and replacement of the current Argon2id hash. Private
password login issues revocable, revision-bound sessions. Rotation preserves the
original expiry. A private guest-order link requires an order-specific proof
issued to the guest basket owner and a current customer session for the verified
buyer mailbox. Private detail and bounded history reads return the linked
order's captured items and amounts under live customer authority. Private
credential admission provides shared purpose and mailbox quotas. A private
session variant binds a synchronizer digest to issuance and protects revocation
under the same session claim. The separately enabled test transport is described
below; public browser ingress and production CSRF policy remain open. Login,
email verification, enrolment and password proof operations commit both
debits before credential work. Password replacement requires an explicitly supplied,
integrity-checked offline blocklist and fails closed on unavailable or expired
snapshots. Private email-verification issuance can now commit the challenge and
its encrypted delivery envelope together. A separate private claim records one
start while that challenge is live. Only a newly committed start can authenticate
the stored envelope and hand a short-lived bearer to a trusted in-process caller.
An optional trusted in-process callback can now record a bounded accepted,
explicitly rejected or uncertain result against that exact start. A start without
a result remains unknown and cannot reopen. No mail transport is installed.
Acceptance does not establish mailbox delivery or redemption. Real corpus review,
coordinated freshness and withdrawal, key and retention operations, message
delivery, neutral browser responses and measured overload behavior remain P4 work.
An optional loopback test-only verification redemption API consumes a supplied
proof after preview authentication, exact Origin validation and shared admission.
It issues no session and remains disabled without explicit test configuration.
Mailbox delivery and public ingress readiness still require acceptance.
An optional private test-only password login uses admitted credential checks
and returns the issued session only in a secure host-only cookie. A separately
opted-in test transport can issue a session-bound synchronizer and revoke that
session with its cookie and proof. The original cookie-only login remains
unchanged. Neither path establishes public login or browser readiness.
A private favourites primitive keeps up to 25 stable merchant SKUs per verified
account. Live session authority and current publication are rechecked; withdrawn
entries are hidden but can be removed or cleared. Browser controls, privacy
export/deletion and production abuse controls remain P4 work.
A separately opted-in test-only GET exposes that same owned projection to a
preview-authenticated caller with the configured Origin and customer cookie.
It can initialize the account's empty favourites bucket; it is not public ingress.
A separate private restock opt-in primitive retains a bounded active subscription
per account and merchant variant; cancellation does not depend on publication.
An independently enabled test-only GET exposes current published subscriptions
to the preview-authenticated session owner and may initialize an empty bucket.
It sends no messages and does not detect stock transitions. Alert delivery,
retention and cancellation policy remain open.

The local room-planner preview combines the rectangle calculator with keyboard
measurement and rotation controls, a drawn footprint, and text clearance and fit
results. Caller measurements stay on the page unless the user downloads a
local file. A selected plan replaces the current draft only after confirmation.
The copy is drafted for all seven locales. The owner approved the 252 Linux
preview baselines with the file controls. Integration requires successful
hosted visual comparison. Native-language acceptance, verified product
dimensions and physical-fit acceptance remain open.

Remaining delivery sequence: P0 business inputs in parallel → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.
The [EU destination readiness register](docs/MARKET-READINESS.md) tracks the
qualified evidence still needed for each of the 27 disabled destinations.

The release check validates committed delivery policy, a clean source revision,
and distinct hashed evidence files through a pinned directory. Human review and
actual provider, deployment, recovery and fulfilment acceptance remain necessary
before activation.

The repository is public and the protection audit passes. Current-revision CI
remains mandatory, including successful evidence uploads, browser acceptance and
populated Sonar/CodeQL results; do not merge based on local checks alone.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.

Guest-basket intent has an opt-in internal session/CSRF API with durable ownership,
revocation, bounded counters and immutable version receipts. No public customer
bridge, order creation or payment follows from it. Owned indicative quote snapshots
now retain exact source revisions, checked totals and short fixed expiry, with
intent-bound retries and final source checks after writes. Historical prices grant
no current order authority. A private promoted snapshot retains exact original
and discounted amounts, the approved promotion, shared issuance limits and
the shorter expiry. Private guest order creation revalidates the approved
terms and debits the lifetime count and cents at acceptance. Private indicative
quotes can also bind a verified customer and an approved per-customer term.
Account-bound order acceptance debits the same verified account under the
promotion lock. Private payment preparation and attempt history use the same
current customer account and guest basket. Private cancellation can close an
owned order before payment dispatch starts, including after an admitted test
attempt. Cancellation shares the basket claim with admission and the attempt
claim with dispatch. Normal hold expiry still governs stock, and lifetime
promotion usage stays recorded. Complete post-dispatch disposition and trusted-edge
abuse controls before exposing customer routes.

Private durable owned-order test attempts and one serialized dispatch-start fact
now compose the frozen request policy, existing physically backed reservation and
minimal transactional recovery events. Fresh commercial and clock checks protect
new work; original receipts remain historical. Runtime dispatch remains uninstalled. Started/unknown attempts must reconcile without automatic replacement.
The private test-key HTTP client now verifies its authenticated profile and
separately fetches checked minimal payment evidence, with bounded one-use permit
transport. Bounded private payment-list pages now return exact metadata-associated
identity hints and validated continuation cursors; every candidate still needs a
separate verified fetch. Empty or incomplete searches never prove absence or allow
another charge. Private verified-fetch observations now preserve minimal original
attempt facts and actual credential fingerprints with immutable revisions and a
processing event in the same transaction. Exact operation receipts replay history;
reordered statuses and different IDs remain evidence without choosing current
payment state. Private authenticated search-page receipts now persist requested/next cursors,
actual credential fingerprints and all candidate identities with atomic outbox
events. Exact continuation, fork/cycle protection and a 100-page bound support
crash-resumable discovery without treating empty/end pages as absence. No credential or running worker is installed. Immutable exact-prefix identity resolution now retains all matched identities and
sticky paid facts, with visible ambiguity, operation receipts and atomic events.
Profile-scoped supervised GET-only recovery now persists leases, search/fetch/poll
checkpoints, bounded retries and explicit review reasons through a local/test
worker command. No runtime key or scheduler is installed. Private finance-authorized transient
recovery now retains immutable full-checkpoint decisions and permits at most three
lifetime resumes without resetting claim, poll or scan budgets. Fresh session/MFA,
exact generation and confirmation protect every call, including receipt replay.
Paid, ambiguous, capped and invalid-custody stops remain unresolved. Private
callback-ID GETs now obtain exact-profile minimal evidence and use authenticated
metadata only to locate and fully match the original frozen order/attempt/dispatch
before atomic observation/outbox custody. No callback body or return status grants
financial authority. Durable private callback lookup operations now precede bound
GETs and recover immutable minimal receipts with dedicated callback processing
events. Unknown/unmatched facts remain visible without an invented order; closed
stored revalidation and complete original matching protect idempotent observation
processing. Private callback intake and GET-only supervision now retain atomic
profile/global quotas, generation/operation custody, fenced checkpoints and
independent receipt processing. An unseeded recent-MFA reversal-review capability
can inspect one exact item review stop with a redacted local summary; it grants no
retry, provider or financial authority. Unresolved older generations retain capacity;
late paid facts stay visible after newer open results or review stops. A private
authenticated API adapter accepts closed bounded classic forms and acknowledges
durable intake only. An optional default-disabled Rust edge validates configured
socket peers and forwards bounded forms to the private API. Private payment
recognition now retains a canonical immutable gross-payment decision per complete
evidence prefix and a separately validated current pointer. Unconfirmed and
ambiguous evidence remains visible; sticky paid facts survive reordering and hold
expiry. New observations make earlier current decisions explicitly stale. Journal,
exact source digest and minimal order event commit atomically. Private original
reservation settlement now consumes a still-intact paid order's complete shared
component allocation once, with a native exact physical proof, immutable receipt
and atomic order event. Private late allocation now recovers intact expired
originals using the same single allocation identity and frozen component demand.
Released or consumed originals and persistent scarcity retain durable review stops;
restock cannot silently restart them. Historical replay preserves original custody.
An unseeded dedicated recent-MFA capability permits private bounded, validated
case summaries. The live cursor requires a refresh for earlier insertions.
A separate private recent-MFA command allows an explicitly confirmed retry of
one scarcity case generation. It retains the original review case and records a
digest-bound allocation or a fresh still-scarce assessment. Release and prior
consumption cases stay stopped. Complete actual proxy/resource-limit/no-bypass
and TLS/provider acceptance before public activation, remaining operator
dispositions and separate order/fulfilment/refund/invoice transitions. A pure
whole-variant quantity check now bounds a shipment proposal against an accepted
order and caller-supplied prior counts. Shipment history, package contents and
dispatch authority still need evidence. Dispatch and payment require separate
authorization.
