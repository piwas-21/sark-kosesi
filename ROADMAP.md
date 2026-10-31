# Implementation status

**Current stage: P3 checkout/payment custody in progress; production disabled.**

The read-only Rust/SQLx catalogue, migrations, typed contracts, API/worker readiness
and courtyard Next.js shells are implemented. Explicit clock/identifier ports
support deterministic diagnostics and generated request correlation. Real
source/coverage/audit, PostgreSQL, component/browser/accessibility, approved visual,
performance and discovery gates are required from the first application PR.
Current-revision CI, independent review and the merge gate remain mandatory.

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
refund allocations preserve captured tax across partial credits. No customer
coupon route, promotion authority, budget claim or refund effect is installed.
Private promotion drafts and independent exact-revision reviews now persist
canonical eligibility/discount terms and explicit lifetime budget/redemption limits.
New drafts and withdrawal block older approvals; no coupons or grants are seeded.
These configuration limits are not spending claims. Private immutable guest orders
now compose accepted quote/seller/market/merchandise snapshots, whole-basket holds
and minimal outbox events atomically. Exact receipt retries preserve history;
changed commercial terms require fresh acceptance. Complete country/territory
readiness evidence, atomic promotion/customer limits, public checkout controls,
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

Remaining delivery sequence: P0 business inputs in parallel → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.

The repository is public and the protection audit passes. Current-revision CI
remains mandatory, including successful evidence uploads, browser acceptance and
populated Sonar/CodeQL results; do not merge based on local checks alone.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.

Guest-basket intent has an opt-in internal session/CSRF API with durable ownership,
revocation, bounded counters and immutable version receipts. No public customer
bridge, order creation or payment follows from it. Owned indicative quote snapshots
now retain exact source revisions, checked totals and short fixed expiry, with
intent-bound retries and final source checks after writes. Historical prices grant
no current order authority. Continue immutable order/address/policy snapshots and
fresh source/amount validation in the atomic whole-basket reservation transaction.
Complete trusted-edge abuse controls before exposing customer routes.

Private durable owned-order test attempts and one serialized dispatch-start fact
now compose the frozen request policy, existing physically backed reservation and
minimal transactional recovery events. Fresh commercial and clock checks protect
new work; original receipts remain historical. No provider I/O or dispatcher is
installed. Started/unknown attempts must reconcile without automatic replacement. Add actual
Mollie transport/fetch verification, workers/callbacks, late-paid stock recovery
and separate order/payment/fulfilment/refund/invoice transitions. A pure domain
plan or matched synthetic response is not dispatch or payment authority.
