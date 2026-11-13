# Engineering architecture

Rust is Piwas's long-term standard for new backend domain services and background workers. This project owns its commerce implementation. Use free, self-hostable open-source dependencies with reviewed licences, reproducible lockfiles and replaceable adapters. Do not introduce a paid licence, mandatory hosted control plane, commerce engine or proprietary runtime dependency. Existing payment/carrier/domain/hosting business costs are separate and require approved provider arrangements.

## Runtime and ownership

| Area                 | Implementation                                                                  | Boundary                                                                                     |
| -------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Storefront and admin | Next.js stable, React, strict TypeScript, server rendering                      | Presentation, form UX and thin server proxy; no duplicated pricing/payment/stock rules       |
| API                  | Rust, Axum, Tokio, serde, tracing, rustls                                       | Authentication, permissions, catalogue, quotes, orders, payments, reviews and admin policies |
| Persistence          | PostgreSQL; SQLx and reviewed SQL migrations                                    | One schema owner; constraints and transactions enforce invariants                            |
| Workers              | Rust binary sharing application/domain crates; PostgreSQL outbox/job tables     | Leases, retries, deduplication, dead letters and reconciliations                             |
| Contracts            | Versioned OpenAPI generated from Rust; generated TS client                      | CI rejects drift; validate untrusted provider and HTTP data                                  |
| Styling              | Tailwind with semantic CSS variables; CSS Modules for complex art/layout/motion | Shared primitives/tokens, logical properties, no arbitrary brand colours in components       |
| Assets               | Piwas-controlled storage and image derivatives behind a storage adapter         | Public product media separate from private uploads; no required image SaaS                   |
| Deployment           | Separate Docker Compose services, Caddy, PostgreSQL                             | Same-origin routing, independent keys/data/backups; no implicit Sofra infrastructure reuse   |

Node is needed for Next.js rendering/builds and JS tooling; it does not own the commerce backend. Use Rust 1.99.1 (2024 edition), pinned Node/npm, stable exact dependencies and committed Cargo/npm lockfiles. P1 verifies framework versions and documents supported upgrade windows. No Redis, Kubernetes, message broker, GraphQL or 3D engine until a measured requirement justifies it.

## Repository layout

```text
apps/web/                 # storefront + protected admin routes
crates/domain/            # money, component inventory and state transitions
crates/application/       # use cases and adapter ports
crates/infrastructure/    # SQLx, Mollie, email, storage
crates/api/               # Axum HTTP/auth/OpenAPI
crates/worker/            # outbox and reconciliation entry point
crates/sqlx-postgres-compat/ # PostgreSQL-only SQLx exports
migrations/              # forward SQL migrations
packages/contracts/      # generated TS client/schema
packages/design/         # tokens and accessible UI primitives
 tests/                   # integration, contracts, browser, security, load
```

Keep modules small and organized by commerce capability. Avoid a generic repository/CQRS framework. Domain code is independent of HTTP/SQL and is deterministic with explicit clock/identifier inputs. Use typed identifiers, checked integer money arithmetic and explicit state transitions; no floating-point money, unchecked casts or panic/unwrap/expect in request paths. Forbid unsafe code in owned application crates. Error responses are typed and redact internals. Connection, request, payload, queue and retry limits are explicit.

Database callers share a small PostgreSQL-only export crate that keeps the
existing `sqlx` import paths. It uses the exact released SQLx core, PostgreSQL
driver and macro crates directly. This keeps the audited lockfile scoped to
the database driver the application uses.

Application `Clock` and `Identifiers` ports supply explicit request-diagnostic
inputs. Infrastructure adapters use checked system-clock conversion and OS
entropy; tests inject fixed values and failures. Each API response has a generated
32-character hexadecimal request ID and Unix-seconds header when those inputs
are available. Caller-supplied IDs are ignored. Unavailable inputs return a
redacted 503; there is no predictable fallback ID. Failure events include only the
generated correlation context, static code and status, with no request URL,
credentials or body. Successful catalogue reads do not generate per-read logs.

## Draft catalogue price history

Catalogue price revisions are append-only rows, separate from the preview read
projection. The catalogue staff guard spans the entire transaction. A caller
must confirm the change, supply the expected prior revision and retain a bounded
pricing evidence reference and reason; the session supplies the actor. A global
effect key serializes exact retries before variant locking. The variant row
serializes revision allocation and a database trigger rejects history edits or
revision gaps, including stale repeatable-read imports. Final clock validation
rolls back prices if authority expires during locks. This adapter adds no HTTP
write route, publication approval, scheduled activation or advertised price.
Existing fixture-only catalogue constraints remain in force.

## Draft product specifications

Each immutable specification revision references a sealed BOM and a closed typed
payload. The domain validates exact component membership/kinds, bounded integer
measurements/tolerances, cover composition in basis points, foam evidence, parcels
and source/traceability references. The staff catalogue transaction records the
session actor, confirmation, reason and expected revision; effect-key retries bind
the full snapshot. Variant locking and a no-op write in the database trigger also
serialize direct imports and reject stale repeatable-read history advances.
Rust bounds the encoded payload at 64 KiB; the database additionally bounds its
JSONB text at 128 KiB. Guarded reads treat stored JSON as untrusted, reject unknown
fields and revalidate it against the immutable BOM before returning staff data.
References are opaque records and are never fetched. This is draft input capture,
not supplier verification, a public content source, fit approval or publishing.
No installed data or HTTP mutation is added; fixture-only identities remain.

## Media registry and permission history

Media metadata is immutable, closed typed JSON with bounded dimensions and source
references. Its SHA-256 digest is unique: a new asset ID cannot relabel known
fixture/generated content as an original product image. Storage keys derive only
from the digest and codec under a private Piwas prefix; arbitrary URLs are never
fetched. Registration does not inspect image bytes or approve their factual use.

Staff grant/revoke commands append scoped permission attestations with explicit
terms, expected prior revision and reason/actor/effect history. Fixture/generated
assets cannot receive product-photo grants, in Rust or the database. Inspiration
stays a separate labelled use. The newest rights event governs; old grant receipt
replays cannot undo a later revocation. A separate serialization row lets metadata
stay immutable while no-op MVCC writes force stale repeatable-read writers to
retry. A deferred FK requires the row, and guarded readers also fail closed if it
is missing. Inspection holds a share claim and evaluates licence/session time
from one fresh authorized timestamp after lock waits. These private consumers
are not publication/image-verification endpoints and do not activate storage.

## Verified private image storage

The image-ingestion adapter has an authorization/read phase, CPU-only processing,
and a fresh authorization/write phase. The immutable registry snapshot must match
on both sides of processing. An unforgeable internal decoded result binds actual
SHA-256, codec, raw geometry and oriented derivative bytes; callers provide no
verified flag. JPEG/PNG/WebP are enabled in pinned image-rs with other formats
excluded. Animation is rejected.
WebP RIFF sizes, chunk ranges/padding and static canvas/native geometry are checked
without payload allocation before decoder construction or metadata access. This
prevents declared metadata lengths from escaping the actual bounded input.
Animation chunks are rejected even when the container's animation flag is clear.
New encoders preserve alpha where applicable and
exclude personal EXIF/XMP/IPTC/comments; originals stay private. Colour, visual
quality, rights and actual SKU suitability still require P0 review.

Two process-wide ingestion permits remain held through storage, and decoder
permits also survive cancellation of an awaiting caller until CPU work finishes.
Overload is rejected rather than queued. Input is at most 20MiB; dimensions obey the registry's 16,384-side/64Mp bounds,
with an additional 128MiB decoded-output check. Derivatives have long-edge targets
320/640/960 without enlargement, each at most 250KiB. Library allocation accounting
is best-effort, so these are not a hard process-memory guarantee; P7 must measure
capacity and enforce process resources. The private PostgreSQL storage boundary
atomically records originals and a complete hash-bound derivative set under a
fixed recipe. No external storage network call or decoding spans a transaction.
Deferred complete-set checks and immutable rows reject partial commits; recipe
identity supports future explicit processing-version expansion. Private reads
validate stored bytes/geometry/metadata and recheck authority after CPU work.
No public original/rendition route, image approval or automatic publication exists.

## Versioned copy and review

Catalogue copy is separate from the fixture preview projection. Closed, bounded
drafts contain title, description, materials, care, safety and assembly fields,
with an exact specification revision. Dutch is the source-copy locale; other
locales also bind an exact Dutch draft revision. There is no legacy-copy or
missing-language fallback. The same variant serialization row used by specification
edits protects copy and review writes. No-op writes provide MVCC conflicts for
stale repeatable-read importers as well as ordinary row locking.

`catalogue.manage` owns drafts; `catalogue.review` is a separate, unseeded grant
for review reads and decisions. Reviewers must differ from the target draft's
author and supply evidence and qualification references with confirmation/reason.
These are auditable attestations, not automated proof of linguistic accuracy or
qualification. Real native-reader acceptance remains P0 evidence. Review decisions
are immutable approval/rejection history; the latest decision for that exact draft
applies. New approvals require current specification, source and target copy.
An independent reviewer can reject historical copy after newer drafts exist,
allowing withdrawal of an older approval. Every decision retains optimistic
review-version checks and fresh authority.
Historical retries preserve their original receipt and cannot restore a withdrawn
approval. Effect identity includes actor, target/version, decision, references and
reason. Authority locks span effects and final clock checks.

Private reads validate closed stored copy and the referenced Dutch source, rather
than trusting a stored reviewed flag. Historical records remain inspectable and do
not establish current publication eligibility. Publication must validate the exact
assembled facts, copy, rights and image approvals and invalidate affected public
projections; those consumers remain unimplemented. There is no public copy or
indexing activation and no new HTTP authoring endpoint.

## Catalogue definition authoring

Catalogue staff can create private merchant families, physical component identities,
variants and sealed BOM revisions. Merchant classification means a candidate for
the business; it does not prove facts or approve sale. The write surface has no
caller-supplied actor, fixture flag, price, URL, stock or publication status.
New company identifiers use bounded uppercase ASCII letters/digits/hyphens and
cannot reuse the DEMO namespace. Supplier identifiers/references remain separate.

Creation events are immutable and bind the complete normalized command, assigned
BOM revision, session-derived actor, supplier/source reference and reason.
Confirmation and recent catalogue authority are mandatory. Exact retries preserve
their original result, while changed actors, evidence or configuration conflict.
Unordered BOM lines are normalized only after collection/string validation.
An entity and its matching sealed BOM/event commit atomically; deferred checks
reject incomplete or mismatched journals. Reads revalidate stored command shapes
and the actual event-to-entity/BOM binding. No-op variant writes also protect
sequential BOM creation against stale repeatable-read importers.

Known fixture identity/classification is immutable. Fixture and merchant family/
component graphs cannot mix, and merchant identities retain their audit history.
Existing fixture rows and legacy readers remain compatible. Legacy item price and
slug are NULL for merchant candidates, avoiding fabricated preview prices/URLs.
The legacy translation table accepts fixtures only, and its reader explicitly
filters fixtures. Merchant public prices, URLs and content require future approved
publication snapshots. No new HTTP authoring route or production activation exists.

Definition creation has no stock effect. Warehouse receiving creates or changes
physical balances under its separate capability; no variant stock pool is added.
Sealed historical BOMs remain explicit reservation inputs after new revisions.
Prepacked stock remains unavailable pending an explicit conversion policy.

## Independent factual review

`catalogue_factual_reviews` is immutable history per exact SKU/BOM/specification/
price tuple. It references private merchant candidates only, with an independent
actor from each source author. `catalogue.review` permits private inspection and
confirmed decisions; it does not grant catalogue authoring. Inspection reuses the
specification and definition transaction loaders, decoding and revalidating stored
source data. Historical inspection is not current publication eligibility.

New approvals serialize on the variant used by source writers and require the
current sealed BOM, specification and price. A specification must bind that exact
BOM. Rejections remain possible for historical targets. Expected review versions
and namespaced effect locks bind the complete intent, actor and evidence. Exact
receipt replay does not overwrite a later rejection; authority is rechecked after
waits before write/read/replay completion. SQL triggers preserve independence,
source linkage, contiguous history and stale-repeatable-read conflicts for raw
imports as well. Installation creates no reviews.

An evidence or qualification reference records a human attestation; software does
not fetch it or establish supplier accuracy or reviewer qualification. The next
publication consumer must check fresh authority/current facts, latest decisions,
reviewed copy, image suitability and rights explicitly. Nothing in this history
activates a public projection or changes the courtyard preview.

## Image suitability history

`catalogue_image_reviews` binds exact SKU/BOM/specification/asset/rights revisions
and the immutable `image-v1` rendition set. Review-only consumers revalidate source
events, measured specifications, closed permission data and hash-bound complete
private image sets. Private inspection returns sanitized derivatives after CPU
validation outside transactions and fresh review authority; it never returns
originals or grants catalogue management. Historical inspection is not eligibility.

Review writes lock the variant, then the media serialization row. The SQL trigger
uses the same order and no-op MVCC writes, rejecting stale repeatable-read imports
after either source or permission changes. Separate review capability,
confirmation, expected revision and full-intent effect identity protect decisions.
The reviewer differs from all product/specification and media/permission authors.
New approvals require the current sealed BOM, specification and product-photo
rights, evaluated with fresh authorized server time before and after the insert.
Licence expiry during an insert wait rolls back approval. Historical rejections
and receipt replay preserve withdrawal without activating old decisions.

No source data, original, derivative or review is seeded. Actual product/colour/
material fidelity and reviewer qualification remain human acceptance evidence.
Future publication must check latest decisions, source freshness and rights again;
no public image serving or production activation follows from an attestation.

## Explicit publication and current eligibility

`catalogue_publication_revisions` records confirmed catalogue-manager decisions
over exact factual, image-suitability and locale-copy review revisions. A translated
selection also pins its Dutch-source approval. Closed commands contain no actor or
eligibility flag. The existing recent-MFA/session guard supplies audit identity and
holds authority locks through the commit. Installation seeds no publication.

Activation locks the variant before the asset serialization row, reuses validated
transaction-scoped fact/image/copy loaders, and requires current source histories,
the selected latest approved decisions and fresh active product-photo rights.
Prices remain integer EUR and must fit JavaScript's exact integer range before
becoming a public selection. The SQL predicate complements complete Rust payload
validation; it cannot alone establish safe public content. No private source bundle
or its evidence/actor/qualification records should become a public response DTO.

`catalogue_family_routes` assigns one immutable family path per locale, unique in
that locale. The first activation and its route commit atomically. Deferred binding
rejects a path without its exact activation event. Variants in a family must use
that assigned path; renaming requires a future explicit redirect policy. Merchant
prices and URLs never enter the legacy fixture-only columns.

Every decision uses optimistic publication history and full-intent retry identity.
Raw imports have contiguous immutable history and variant/asset MVCC writes.
Final clocks roll back both activation and path assignment after licence/session
expiry during waits. Deferred constraints run before the last clock sample, and
the locked permission snapshot is checked without another database wait before
commit. Withdrawal retains history and paths. Receipt replay returns
the original revision without changing later decisions and is not eligibility.

Private status claims the variant before selecting the latest publication, so a
withdrawal committed during a wait cannot be missed. It validates sources and
latest review decisions under the variant/asset claims and samples a fresh clock.
The assigned path's original activation binding is revalidated on reads and reuse;
the final clock is sampled after the eligibility query finishes.
Changed drafts, withdrawn/reapproved reviews and revoked/expired/replaced rights
invalidate an earlier activation; there is no older-source or approval fallback.
A new explicit activation must consume the new approved versions. Public serving,
safe derivative access, availability projections and cache/HTML/feed agreement are
still separate P2 work; these records enable no production route or sale.

## Inventory and finance

Model sellable variants separately from physical components. A configuration has a versioned bill of materials: seat/back/bolster covers and optional matching foam inserts. Covers-only does not reserve foam. Prepacked and separable stock require explicit warehouse rules; never count both kit stock and its contents as independently available.

Maintain append-only stock movements, on-hand/quarantine/reserved quantities, reservation ownership and expiry. Reserve all components atomically in deterministic locking order. Database constraints prevent negative usable balances; unique keys prevent duplicate event application. Test competing orders against real isolated PostgreSQL. Receiving, returns inspection and release of reservations are distinct movements.

The component catalogue expands the P1 preview schema without changing its read
contract. A variant links one SKU to a product family, size/textile codes and an
explicit separable stock policy. Cover/foam physical component codes are shared
between variants. Draft BOM lines become immutable when the revision is sealed;
database triggers serialize edits with sealing and reject empty, oversized or
fill-inconsistent revisions. Component identity/kind cannot be rewritten and a
sealed revision fixes the variant's fill. The Rust adapter loads only sealed
revisions in component-code order. Prepacked kits and non-fixture catalogue data
remain disallowed pending their owning policies. These tables add no admin write
endpoint and do not publish merchant facts.

Warehouse movements use the deterministic `StockBalance` domain value. Receiving
and counts change physical on-hand quantity; quarantine/release change usability;
disposal removes inspected quarantine from physical stock. None changes reserved
stock or implements a refund. The SQLx primitive locks one component balance,
validates the transition and writes both balance and append-only deltas in one
transaction, with bounded database waits. Effect keys are globally unique and bind
component/action/quantity/actor/reason; exact replay returns the stored receipt.
Cross-component key collisions roll back the losing balance. Ledger timestamps
are PostgreSQL transaction audit time, never inputs to domain transitions. This
trusted persistence primitive has no HTTP entry point; future warehouse use cases
must supply server-authorized identity/capabilities and the P0 batch procedure.

Hold creation stores an immutable owner, SKU/BOM revision, unit count and explicit
clock window (1–1800 seconds), then reserves all snapshot components in code order
in the same transaction. A building hold cannot commit; activation requires an
exact sealed-BOM snapshot. Hold identity binds retries, and each component effect
has a unique hold/action reference in the movement ledger. Terminal transitions
carry an explicit server actor, separate from the hold owner, so worker effects
are never attributed to a customer merely because they own the hold. Release and expiry
free reserved units; consumption removes both physical and reserved units. The
hold row serializes competing terminal transitions. Consumption at or after the
deadline fails; P3 must explicitly reallocate late paid orders and surface failure.
These are trusted primitives without HTTP exposure. The worker's explicit
`--expire-holds` command captures a clock and processes at most 1–100 holds per
pass (25 by default). It claims active due rows with `FOR UPDATE SKIP LOCKED` and
uses the same transition policy as owned release/consumption.
The claim transaction explicitly uses read committed and rechecks retry eligibility
in a fresh statement under the hold lock, so a cooldown committed during candidate
selection cannot be bypassed, including on connections defaulting to repeatable read.
A per-hold savepoint
rolls back failed balance/ledger effects without releasing the claim, then records
bounded retry state with a static code and a 60-second delay. Successful expiry or
another terminal transition marks the retained failure record resolved. Crashed
transactions roll back; another pass can safely recover. The default probe is
read-only, and both modes reject production configuration. Real expiry supervision
and session/capability adapters remain required before operational use. P3 owns
outbox processing for external effects; hold expiry performs only database work.

The Rust quote service computes item/discount/tax/shipping allocations in integer minor units and a single currency (EUR at launch). Persist immutable order line, bundle, price, VAT, discount, address and policy snapshots. A later catalogue edit cannot rewrite order history. Discounts have explicit eligibility, budget, per-customer limits, stacking and time-zone rules; concurrent redemption cannot exceed a limit.

### Atomic basket reservation manifests

The trusted basket primitive accepts at most 20 exact sealed-BOM selections,
stable child/group identifiers, an immutable server order identity and one
bounded hold window. It creates an immutable manifest and fresh child holds in
one transaction. Child bindings are fixed at insert time; legacy holds cannot
be adopted, and another group cannot share a child. Deferred schema guards
reject open, incomplete or mismatched manifests. Installation seeds no groups,
holds, stock or customer authority.

Admission and completion clocks prove new holds remain inside the domain's
consume window and reject rewind from the latest observed time. All child IDs
are inserted in a common order, and missing-balance insertion plus UPDATE claims
use the same database component order as existing stock transitions. Aggregate
shared demand is checked before per-SKU ledger effects. There is no quote SHARE
claim upgraded into a reservation; every selected physical component is claimed
before effects, avoiding cross-SKU lock cycles. Scarcity, failure or cancellation
rolls back the manifest, holds, snapshots, stock and movements together.

Exact retries bind the full owner/window/member selection and return historical
current child states in canonical SKU order, with no reactivation or new stock.
The immutable manifest remains compatible with per-hold expiry; future payment
consumption/release must use one caller-owned transaction for the entire order.
The internal transaction entry does not commit: the checkout caller supplies a
fresh read-committed transaction, bounds and final authorization/source/window
checks alongside order, promotion claims and outbox writes. Public customer
routes, order creation and payment recovery remain separate P3 integration work.

### Indicative server quotes

The trusted quote consumer accepts a closed destination/SKU/quantity request;
prices, VAT, shipping and readiness are server inputs. No market policy is
installed. The opt-in private guest quote routes below use persisted reviewed
policies; the trusted primitive grants no customer policy authority. A policy explicitly binds one EU country and exact
postcode to eligible publication revisions, quantity ceilings, item VAT rates
and one basket shipping price/rate with parcel count, total weight and edge
limits. Exact matching has no country-wide, territory, sibling or draft fallback.
Policies are trusted server configuration values with identity, revision, evidence
reference and validity window, not proof of carrier/accountant approval. The full country/territory readiness matrix and verified inputs remain required
before sales.

Private durable policy administration uses unseeded `market.manage` and
`market.review` capabilities. One immutable anchor binds a policy ID to one exact
EU country/postcode, with a unique destination. Confirmed, optimistic drafts retain
closed validated policy JSON and session-derived actor/reason/effect history.
Independent reviews bind an exact draft and evidence/qualification references.
New approvals require the latest draft; historical rejections and exact receipt
replays preserve later decisions. References are attestations, never automated
verification of tax, carrier, territory or return obligations.

The reviewed quote consumer holds the policy anchor through the same transaction
as catalogue and physical stock claims. Only the newest draft with its newest
approval is usable; there is no fallback after edits/rejection or ambiguous
country-wide inference. Read boundaries revalidate stored policies and reviews.
No-op anchor writes serialize history and reject stale repeatable-read imports;
database triggers also reject immutable-history edits, gaps, self-review and stale
approval. Staff clocks are checked after writes/reads and effect waits. No policy,
grant, country-wide readiness, HTTP checkout or payment is installed by migration.
The existing trusted in-memory quote primitive remains available for isolated
adapter tests; customer-facing checkout must use reviewed persisted policy state.

A single read-committed transaction reuses publication validation and claims all
variants before assets. It sums demand across SKUs for shared physical components,
then claims balances in component-code order, accounting for quarantine and prior
reservations. Missing balances fail closed. Quote reads never write stock or
financial records. After all database waits, one fresh clock rechecks policy and
all captured product-photo rights; backward/unavailable clocks fail closed. Source
and review changes remain excluded by the held publication claims.

Amounts are VAT-inclusive EUR integer minor units, checked against the exact
JavaScript integer ceiling. VAT rounds half up once per extended SKU line and
shipping; net is gross minus that allocation. Totals sum line/shipping allocations,
including mixed explicit rates. Canonical SKU order makes equivalent baskets
stable. This is the initial deterministic arithmetic policy, subject to actual
accountant/market acceptance; promotion/refund arithmetic is described below.
A quote grants no guaranteed price window or reservation authority.
Future checkout must revalidate and capture immutable sources, address and policy,
then reserve the entire basket atomically before creating a payment attempt.

### Indicative promotions and captured refund allocations

A trusted optional promotion can now price the same reviewed market quote under
its existing policy, publication, image and physical-stock claims. Promotion terms
are closed, bounded server values; no customer request can supply them, and no
promotion or coupon route is installed. Exact publication/SKU and country scopes,
a minimum eligible item subtotal, explicit maximum discount and UTC instants apply.
A quote supports one promotion, without stacking or shipping discounts. Private
promoted order acceptance records the lifetime discount and redemption claim
inside the order transaction.

Allocation version 1 rounds a percentage once, half up, on eligible extended-line
gross. Fixed or percentage discounts are capped by their explicit maximum and the
eligible subtotal; a zero benefit is ineligible. Largest remainder distributes
whole cents by eligible line gross, with ASCII SKU order for ties. VAT then splits
each discounted extended line using the existing inclusive rounding rule. Shipping
keeps its original allocation. Results retain original unit/extended prices,
per-line discount and final net/VAT/gross, promotion revision and allocation version.
The final clock runs after all pricing and stock waits and rechecks market,
promotion and photo-rights eligibility together.

The pure refund allocator uses the immutable captured gross/net/VAT snapshot.
For a partial gross refund it subtracts the previous cumulative rounded VAT share
from the new cumulative share. Full repayment therefore returns the exact captured
VAT and net across any partition of refunds, without repricing current catalogue
or repeatedly rounding isolated credits. This calculation neither authorizes nor
executes refunds. The later finance use case must serialize and persist already
refunded gross, enforce captured/refunded bounds, issue credit documents and use a
stable provider attempt. Accountant/market acceptance remains a separate gate.

A pure order-scoped allocation composes that arithmetic over the validated accepted
order quote. It requires one explicit prior gross and requested gross for every SKU
and shipping, rejects duplicate, unknown, missing and excessive allocations, and
returns line, shipping and total gross/net/VAT deltas from the amounts captured
after any promotion.

### Private refund reservations

An explicitly confirmed reservation binds an accepted order, the expected
current received recognition, the requested gross for every accepted SKU and
shipping, the staff account, reason and a global effect key. Zero line amounts
are valid when the total request is positive. Staff requests use the unseeded
`refund.prepare` capability and recent MFA. The transaction claims the original
payment attempt in READ COMMITTED and verifies the received evidence against the
accepted EUR amount after the claim. Recheck staff time after waits and writes.

The attempt claim serializes each order's immutable reservation history. Retain
at most 16 reservations per order. Revalidate previous custody before deriving
cumulative reserved gross and computing each net/VAT delta from the captured
snapshot. PostgreSQL independently enforces the same cumulative bounds and
cent allocation. Commit the reservation and a reciprocal minimal event in the
same transaction. An exact replay under current staff authority returns the
original reservation and checks its historical recognition. New evidence can
block a new reservation without changing an existing receipt.

A private test-key reconciliation client can fetch one exact refund or chargeback
only after an authenticated GET matches its original paid Test/EUR payment to
the frozen order, attempt, profile and amount. The bounded child read validates
its identity, parent, positive EUR amount, documented refund phase or typed
chargeback reversal timestamp, and retains only minimal in-memory provenance.
The chargeback schema has no required mode. Test scope derives from the verified
parent and test credential. An omitted reversal timestamp leaves reversal status
unknown. Unknown responses and 404 leave the child unresolved.
The project follows Mollie's [refund GET](https://docs.mollie.com/reference/get-refund)
and [chargeback GET](https://docs.mollie.com/reference/get-chargeback) contracts.

A separate GET-only test-profile operation reads the optional aggregate fields
from the [paid parent payment](https://docs.mollie.com/reference/get-payment)
after exact attempt, profile, payment identity, amount and metadata matching.
It preserves reported refunded, remaining, captured and charged-back amounts
as EUR minor units. Omission remains distinct from zero, and reported refunds
above the original stay visible for review.

The trusted operation commits against the exact current received recognition,
observation watermark, payment and actual test credential before I/O. An opaque
permit can start one GET within 60 seconds. An exact effect replay restores the
pending operation or its receipt. The result is bound to the historical operation
and can retain a later response from a GET that started in time. The immutable
amounts and a minimal digest event commit together under the original attempt
claim. They supply evidence for later reconciliation. Net balance and refund
execution require separate authority and evidence.

A read-only set inspector claims the attempt, validates the current received
recognition and its complete observation prefix, then reuses that native prefix
proof for each distinct historical recognition in the bounded operation set.
It checks operation shape and source, receipt shape and reciprocal event, and
hashes every full operation/receipt state in operation-ID order with the attempt
and current recognition watermark. Omitted amounts remain distinct from zero.
The detached digest is not a continuing lock, provider completeness, balance or
financial authority; a later start, receipt or recognition invalidates it.

A private list operation verifies that same paid parent before each bounded GET
of its refund or chargeback collection. It returns only exact candidate identities;
item status has no authority. The continuation is an in-memory capability bound to
the entire frozen attempt, payment, collection and actual test-key fingerprint.
Only a validated Mollie next-link offset is used to reconstruct the fixed endpoint;
the returned URL is never followed. Empty, ended, malformed or capped scans do not
establish absence or a net balance. The contracts are the [refund list](https://docs.mollie.com/reference/list-refunds)
and [payment chargeback list](https://docs.mollie.com/reference/list-payment-chargebacks).
The private observation journal accepts the exact authenticated item result
after network I/O. It locks the dispatched attempt, compares the full frozen
source and appends an immutable fact with the actual test-key fingerprint. It
preserves signed source seconds, nanoseconds and the distinction between unknown
reversal status and an observed reversal. Each attempt has at most 128 receipts.
The original effect key replays only the identical intent. A reciprocal outbox
event carries the order, attempt and observation identifiers and a SHA-256 digest
of the complete immutable receipt in the same transaction. Reads verify that
binding before returning a receipt. Observations retain each
provider identity for later reconciliation, including ambiguous payment matches
and amounts above the original payment. Financial and inventory transitions are
outside this evidence path.

The private page journal records a verified collection response after network I/O.
It claims the dispatched attempt, preserves the provider's ordered candidates and
binds every stored field to a reciprocal outbox digest. Each root starts a separate
scan. A successor must consume the exact predecessor offset under the same attempt,
payment, collection and actual key fingerprint. A predecessor has at most one
successor. Repeated offsets fail closed. The journal keeps at most 32 pages per
scan and 128 per attempt. Exact effect replay verifies the historical receipt
and its ancestry. After a restart a validated, open scan tip can restore an opaque
continuation. Key rotation requires a fresh root. Terminal and capped scans leave
history unresolved.

The private page supervisor enrolls one active scan per attempt, payment,
collection and actual test key from a validated paid observation. Each attempt
retains at most 16 scan jobs, and each scan retains at most 40 operation
generations. A short database claim commits one exact
start and a 60-second lease before network I/O. One operation can verify the
parent and fetch one page. Each explicit batch accepts at most 25 steps from a
client whose profile was already verified. The checkpoint advances only with
the exact operation and reciprocal receipt. An expired operation can recover an
already recorded page without another GET. Expiry without a receipt parks the
scan for review and preserves any late page as separate historical evidence.
Transport failures that return to the worker retry after 60 seconds and stop
after three consecutive failures.

The independent item supervisor enrolls one exact candidate ordinal from a
validated page and its authenticated paid original. The job freezes the actual
test-key fingerprint, so a newly verified key uses evidence from its own scan.
Separate pages can retain separate observations of the same child. Each attempt
can hold up to 128 item jobs and 128 exact observations. Each job has at most
three operation generations with the same 60-second lease and backoff. One
operation fetches the paid parent and exact child, then appends the authenticated
fact to the observation journal under its operation ID. A validated receipt
ends the job in the observed state. Recovery can use that exact receipt after
expiry, while an expired start without a receipt stays in review and preserves
any late observation. The job records evidence for reconciliation and reservation
disposition, which remain separate operations.

Reservations encumber refundable amounts. Before execution, reconcile
provider-side refunds and chargebacks, define uncertain and failed attempt
dispositions and use stable provider idempotency and credit-document links.
No inventory movement is part of the reservation. Accountant, market and
provider acceptance remain separate gates.

### Private refund execution proposals

A separate command records one immutable test-mode proposal per reservation.
Staff explicitly confirm the order, reservation, expected current received
recognition and observation watermark, with a reason and effect key. Under the
shared payment attempt claim, bind the reservation digest and its exact captured
SKU and shipping allocations to the sole recognized paid identity under the
original frozen test profile. A newer received recognition may support a new
proposal without changing the reservation. The proposal and its minimal
digest-bound event commit together. Exact same-effect replay revalidates the
historical source and requires the original actor and current staff authority.

The proposal is an audit record for later review. Execution still requires
provider-side refund and chargeback reconciliation, a separate authorized
decision and durable dispatch and recovery custody. This command has no provider
I/O or stock, refund disposition or credit-document effect.

### Reviewed promotion configuration

Private promotion policy storage uses separate unseeded `promotion.manage` and
`promotion.review` capabilities with the existing persisted-session/recent-MFA
guard. A different account reviews an exact draft. Confirmed mutations preserve
explicit EUR currency, canonical country/product selection, fixed/percentage terms, UTC validity,
explicit total discount/redemption ceilings and an optional per-customer limit.
Stored JSON must include the nullable customer-limit field; an explicit null
records the absence of that limit. A missing field fails validation. Limits are
lifetime terms for the promotion identity and carry forward across revisions.

Immutable anchors and real no-op writes serialize draft/review history, including
stale repeatable-read imports. Effect retries bind full canonical terms, actor,
reason, revision and decision without restoring old approval. The current consumer
selects the latest draft and its latest independent approval under an anchor SHARE
claim; missing, withdrawn, expired or ineligible state has no older fallback.
Bounded stored JSON requires closed object shapes, exact identity, valid limits
and canonical selections. Review-only inspection can retain historical sources.
The trusted configuration reader is not an HTTP route or fresh quote authority.

The shared observed clock supports both owned and borrowed clock sources. One
operation tracks every sample through staff authorization, source waits and final
checks, without extending a lifetime or clamping a backward observation. Authority
claims survive all writes; cancellation and final clock failures roll back history.
No source review, provider request or network side effect is implied by a receipt.

Private order composition claims the owner and basket, market, exclusive promotion
anchor, sorted catalogue sources and physical components in that order. The
promotion anchor serializes current review changes and capacity use across buyers.
An immutable redemption per accepted order carries the exact approved source,
gross discount, lifetime sequence and cumulative discount. The latest indexed
receipt supplies the prior usage across policy revisions. A database constraint
requires the matching debit to commit with the order, holds and event. Per-customer
limits require verified server identity and remain unavailable to guest checkout.
Capacity stays used after a hold expires until a separate disposition policy is
implemented.

### Non-selling HTTPS staging

`APP_ENV=staging` is a preview environment, separate from production. The Next
renderer accepts an explicitly configured HTTPS DNS canonical origin and an exact
40-character revision; local/test origins retain their loopback requirement. The
Rust API still binds loopback and keeps its existing bearer, payload, timeout and
capacity controls. Preview responses, including challenges and redirects, carry
no-store/noindex/no-referrer/nosniff and the staging revision. Request Host and
forwarded headers never select the canonical redirect origin.

The pinned multi-stage images run as an unprivileged user. The API shares the web
container's network namespace, permitting a loopback-only API without publishing
its port. PostgreSQL uses a separate internal Docker network and private volume;
there is no public database port. Only the web port joins the
existing TLS proxy network. Ingress starts disabled until review/CI and isolated
startup pass. Separate credentials, bounded resources, read-only application
filesystems, image tags and runtime revision bind the preview to accepted source.
No production data, staff authority or merchant/fixture records are installed.

Staging can explicitly opt in to public storefront reads with
`STAGING_PUBLIC_STOREFRONT=true`. An allowlist covers existing GET/HEAD storefront,
customer catalogue/image and discovery routes; unknown paths and mutations keep
the browser gate. The `test-admin` browser credential is a distinct random
`STAGING_ADMIN_TOKEN`, never the internal API token. It only gates the unavailable
admin shell and grants no staff capability. Local/test previews retain their
original authentication. Full staff login/MFA/session issuance remains P4 work.

## Payments and durable work

Use a dedicated approved Piwas retail website profile and keys in the existing Mollie organization only if Mollie confirms eligibility. Do not copy Sofra's subscription keys or payment logic. Start with implemented payment methods only; hosted provider checkout reduces card-data scope. Customer redirects never establish payment success.

Create a stable payment attempt and idempotency key before calling Mollie. Do not hold a DB transaction during provider I/O. Unknown create/capture/refund outcomes retain their original attempt/key and enter reconciliation; never mint a replacement charge. Fetch webhook identifiers with the correct server credentials and verify order, amount, currency, environment and profile. Deduplicate and tolerate delayed, repeated and reordered notifications.

A private exact reversal-item review inspection holds an unseeded dedicated
`reversal.review` staff grant and recent MFA through the existing attempt claim.
It revalidates the paid source, page, complete bounded operation history and
each exact-effect late receipt, then rechecks staff time before returning only a
closed reason, generation and whether any late receipt exists. No provider
identifiers, network access, state changes or financial or recovery authority
follow from this detached summary. Migration 0064
follows separately reserved migrations 0062 and 0063.

Order, payment, fulfilment and return states are separate. Late paid payments after reservation expiry attempt atomic reallocation; failure creates a visible finance/support exception and recovery/refund process. Payment success cannot be discarded because stock expired. Partial refunds cannot exceed captured less refunded amounts; preserve tax/discount allocation and credit-document links. A refund does not automatically restock goods. Chargebacks are finance events.

A pure shipment quantity check validates the accepted order and accounts only
for complete sellable variant units. Its caller supplies a complete prior count
for every captured SKU and a positive proposed count for each selected SKU. The
check rejects duplicates, unknowns, incomplete prior counts, overflow and totals
above the original ordered quantities; output follows captured quote order.
This is arithmetic over supplied facts, not a shipment receipt or authority to
pick, dispatch or consume stock. A later fulfilment transaction must authenticate
staff, lock and reconstruct complete committed history, bind idempotent effects,
revalidate order/payment/stock state and persist its event. A variant may need
multiple physical packages; individual components cannot be claimed as a whole
unit without separate package and component custody.

Write domain changes and outbox events in one transaction. Workers claim leases using PostgreSQL, retry with backoff and stable effect identifiers, and recover abandoned leases. Emails, stock alerts and fulfilment requests are idempotent. Record provider acceptance separately from customer delivery. Failed jobs are visible and replayable only through bounded, audited admin actions.

### Initial test-attempt model

The private version-1 payment domain captures immutable server/order/basket/stock
identities, positive bounded EUR minor units, explicit test mode, profile and
credential fingerprint, a configuration digest, a frozen-request digest and an
active dispatch deadline. Validated request-independent bindings are separate
from the final frozen snapshot, so construction needs no placeholder hashes.
Its idempotency key derives from the immutable attempt identity. No key, request,
profile, amount or readiness is accepted from a customer by this model.

The pure dispatch policy requests a single Prepared-to-Started transition.
Prepared plans reject changed profile, credential or configuration identity.
Infrastructure must durably serialize that transition before any provider I/O;
a pure plan supplies no dispatch authority. Started means the request may have
reached the provider. Every subsequent plan reconciles, including after credential
rotation, hold expiry or the provider cache window. A known provider ID is immutable
and does not imply payment success. Initial automatic POST replay is deliberately
absent; uncertain outcomes remain visible until reconciled, never replaced blindly.

A server-fetched payment projection must match resource, provider/profile identity,
mode, exact EUR amount and order/attempt metadata. Version 1 uses closed test/EUR
contracts with no missing/unknown fallback. Matching retains late-paid evidence
without consuming stock or changing order/payment/fulfilment state. Provider IDs
are bounded opaque prefixed values, accommodating longer upstream identifiers.
Identifiers and credential custody data have no Debug implementation.

The application freezer now captures canonical request bytes, exact decimal EUR,
configured supported-method syntax, oneoff sequence and order/attempt metadata.
Return/callback URLs must be distinct canonical HTTPS routes under one explicitly
configured DNS origin, without user info, query or fragments. Return identifiers
are added by the server. No profileId/testmode parameter, raw key or contact/address
data enters the provider request. Stable configuration and request digests bind
these bytes; restore requires exact canonical reproduction under the captured
policy. No active policy, method eligibility or deployment is installed.

Private PostgreSQL preparation now binds one immutable attempt to one owned
accepted order and its captured EUR total, original stock group, exact canonical
request bytes and configuration digest material. Native SHA-256 constraints bind
the stored UTF-8 bytes. Fresh guest ownership/CSRF protects preparation; only the
original effect and confirmed intent replay its receipt. New preparation rechecks
current reviewed market, publication, merchandise, photo rights and seller policy,
then claims the complete original active reservation and its physical backing.
The accepted order may outlive the original indicative quote and current cart;
its own reserved components are never counted as unavailable indicative stock.
No new hold, movement, cart clearing or source activation follows from preparation.
The attempt and minimal prepared event commit together, with final combined
source/hold/identity clock checks after the actual event write. Private history
revalidates bounded closed records, canonical request/configuration and the
original accepted order; reading old history grants no new payment authority.

For an account-bound order, private preparation and history hold the current
customer account and session before claiming guest and basket ownership. The
attempt retains the quote's exact account in a private immutable column. Scoped
reads and effect replay authorize the original order before decoding its frozen
payment request. The final observed time covers both sessions and the existing
source and stock deadlines. Internal dispatch and reconciliation validate the
retained attempt and order association when restoring history.

The trusted internal dispatch entry serializes contenders on the immutable attempt.
A new start requires the same configuration and fresh commercial/held-stock claims.
One immutable start and a reconciliation-required event commit before a noncloneable,
nonserializable dispatch permit is returned. Permit consumption checks the final
observed clock minimum, configuration, stock deadline and retained market/photo/
seller time limits again, after database claims have ended. The permit retains
only the pure eligibility proof and seller expiry, without a transaction or buyer/
address snapshot. Existing starts return reconciliation, including after guest
revocation, policy withdrawal, hold expiry or key rotation. Losing a permit or
crashing after commit cannot create another initial POST. SQL uniqueness and
deferred event completeness provide durable custody; no provider transport or
running dispatcher is installed.

Complete authenticated fetch/reconciliation, late-paid stock recovery and separate
finance/fulfilment transitions before using it for payments.
Actual profile/method/key and sandbox acceptance remain independent requirements.

### Private test-provider transport

The explicit test credential accepts only bounded `test_` keys and binds a
versioned domain-separated SHA-256 fingerprint to the configured profile. Raw
keys have no Debug/serialization/getter and survive only in sensitive HTTP
Authorization headers. An unverified client has no payment operation: an
authenticated `GET /v2/profiles/me` must match the expected profile before it can
become a verified client. No environment provisioning or worker activation is
installed by the library.

Production construction fixes the HTTPS Mollie API origin. The HTTP client uses
an explicit Ring/Rustls configuration and reviewed WebPKI roots, HTTP/1, no system
proxy, redirects, automatic retries, referer or decompression, and bounded connect/
read/total deadlines. Responses are limited to 65536 bytes under both declared
length and streamed framing. A loopback override exists only in unit-test builds.

Create consumes the durable one-use permit and sends its exact frozen body/key
after all database claims have ended. Every unusable create reply remains
uncertain. A matched create response supplies only an identity hint; a separate
authenticated GET must verify resource/provider ID, profile, explicit Test/EUR,
checked decimal minor units, exact order/attempt metadata, status and oneoff
sequence. Only the minimal projection survives; bank/card/address details are
ignored. Unknown statuses and malformed critical objects fail closed. Optional
checkout links require canonical HTTPS and an explicit Mollie-host allowlist;
invalid links are withheld without discarding matched financial evidence.

A GET has no current hold/source deadline and can use a rotated test key whose
profile was independently verified against the SAME captured profile. Its actual
credential fingerprint remains available for observation audit. Rotation never
authorizes another POST under the old attempt: initial permits still require the
original complete configuration identity. This client persists no provider ID or
payment state. Supervised search jobs, identity resolution, callbacks, late-paid
stock recovery and separate finance/fulfilment transitions remain required.
Loopback instruments are not provider sandbox acceptance or configured credentials.

Private unknown-ID discovery performs one authenticated list request per explicit
call, with a fixed ten-payment limit, descending order and the existing byte/time
bounds. It sends neither profileId nor testmode with the profile-bound test key.
List results return only possible identities: each must undergo the separate
authenticated GET before financial observation. Two matching identities remain
visible; the client never silently selects a winner. Empty results or a missing
next link never prove absence, end uncertainty or authorize another create.

Explicit guards require embedded/link containers, each payment entry and non-null
next links to be JSON objects; positional arrays cannot substitute for structs.
Typed raw JSON entries preserve duplicate critical fields through metadata
selection and full payment validation. Unrelated profile payments may carry
arbitrary metadata; either matching order or attempt identifier requires the full
exact binding, so partial matches fail closed. Page counts, a maximum of ten
entries and duplicate provider identities are checked. Continuation links must be
canonical HTTPS under api.mollie.com/v2/payments, at most 2048 bytes, with one
bounded from identifier, limit=10 and optional sort=desc. Other/duplicate query
parameters, user info, fragments, ports and a repeated current offset fail closed.
Only the decoded cursor survives; the next request is constructed against the
fixed provider endpoint instead of following the supplied URL. There is no automatic pagination or job runner in this transport. The private
store below now retains authenticated page evidence.

### Private fetched-observation custody

The trusted observation store accepts only the opaque authenticated GET result,
never callback/redirect JSON or a caller's amount/status projection. It restores
the immutable original order/attempt and committed dispatch, rechecks the full
provider binding, and captures the actual fetched credential fingerprint, including
a separately verified rotated same-profile key. No raw key, response details,
address or checkout link enters this minimal financial evidence record.

An explicit read-committed transaction and attempt-row UPDATE claim serialize
append and full-intent effect replay, including when the connection default is
repeatable read. Reads after a claim wait must see facts committed during it.
Exact receipts retain their original revision/time even when clock or entropy
ports are now unavailable; changed status, identity or credential evidence under
that same effect conflicts. Workers must look up that operation's receipt before
refetching on retry and use a new effect for a new fetch operation. New records
require checked contiguous revisions and nondecreasing server operation time,
including a final observed-clock check after the actual event write. The timestamp
is neither provider paidAt nor an exact commit timestamp. No current stock/source/
guest/configuration expiry discards genuine matched late-paid evidence.

The minimal immutable observation and PII-free processing event commit atomically.
Original order/prepared/recovery event kinds retain their per-order uniqueness;
observation events instead have one unique observation identity. SQL guards bind
closed snapshots to captured attempt money/profile/metadata and dispatch-start records,
serialize raw appends, and defer event completeness until commit. Bounded private
reads revalidate record shape, original binding, headers and exact event presence;
a damaged/oversized/orphaned receipt fails without falling back to older history.
Reads expose individual historical facts, not a certification of the entire journal.

Recording a fact neither binds a single provider identity nor chooses current
payment state. Reordered statuses and different matched provider IDs remain
separate evidence. No order/stock/payment/fulfilment/refund transition, public
customer read, queue consumer or runtime credential is installed. Future identity
resolution and idempotent transitions must preserve paid evidence and surface
ambiguity; callbacks, supervised jobs, late-paid allocation and provider acceptance
remain required.

### Private discovery-page custody

Authenticated list calls now return an opaque page bound to the entire original
frozen attempt, requested cursor and actual verified credential fingerprint. The
private store compares that complete binding with the restored original accepted
order/attempt and requires a committed dispatch. Listing and database work remain
separate: no transaction crosses provider I/O. Page candidates remain hints for
separate authenticated GETs, including when more than one identity matches.

Each operation has a full-intent immutable receipt and a minimal processing event
in the same transaction. Receipt lookup precedes a retried list operation; exact
replay needs neither a clock nor entropy, while a changed page needs a new effect.
Read-committed attempt claims serialize global contiguous revisions even under a
repeatable-read connection default. New writes enforce nondecreasing operation
time and sample the observed clock after the actual outbox insert. Current guest,
source, hold or key expiry does not discard historical discovery evidence.

A continuation binds one exact preceding page and its returned offset. One
predecessor has at most one continuation; mismatched, terminal, cyclic and forked
paths fail closed. A scan contains at most 100 pages; its last cursor remains
visible when that bound is reached. Independent later scans can start at the first
page with a new operation identity. Neither an empty page, end link nor bounded
scan establishes provider absence, permits another create or resolves uncertainty.
Workers still need durable scheduling/leases and explicit operational escalation.

Native constraints preserve minimal closed snapshots, ten unique bounded candidate
identities, actual key fingerprints, immutable history, contiguous revision/time,
exact predecessor custody and deferred outbox completeness. Bounded private
readers restore the original attempt and validate every ancestor and its exact
event, rejecting damaged, oversized, missing or cyclic history. These are private
trusted-worker primitives; no callback, customer route, queue consumer, credential
provisioning, payment-state transition or runtime provider call is installed.

### Durable payment identity resolution

Private resolution claims the original dispatched attempt and revalidates the
complete contiguous authenticated-observation prefix. Its immutable receipt binds
an explicit watermark, sorted distinct matched provider identities, and the set
of identities with any paid evidence. Zero identities remains unresolved; multiple
identities is visible ambiguity, never a selected winner. Paid evidence survives
later open/pending/failed observations and remains attached to every identity.
These are historical facts, not a current payment status or fulfilment authority.

Each test-profile/provider-ID pair can belong to only one original attempt.
A unique immutable database claim binds its initial resolution, original attempt
and exact profile. Native uniqueness forces stale repeatable-read writers to abort
as well as serializing ordinary application claims. A conflicting attempt retains its fetched evidence
and fails resolution for explicit recovery; different profiles remain isolated.

Resolution and its minimal outbox event commit together. Native guards independently
derive the exact summary and current watermark, preserve immutable contiguous
resolution history and reject missing events. Bounded readers recalculate the
captured prefix and validate exact event custody, without falling back after
corruption. Receipt replay retains its original prefix, even after later evidence,
clock failure or credential/stock expiry. New decisions require nondecreasing time
and a final clock check after writes. A 1,000-observation bound fails visibly rather
than truncating evidence; further recovery requires an explicit operator decision.
Supervised processing, callbacks, separate financial transitions, late-paid stock
recovery and provider acceptance remain required before operational use.

### Private payment recognition

The existing unique immutable attempt per order remains the financial source.
A private recognition journal records separate gross-payment state without
rewriting accepted checkout snapshots. The complete authenticated observation
prefix and its closed identity resolution determine exactly three states:
`needs_review` when more than one matched provider identity exists; `received`
when exactly one identity has any paid evidence; otherwise `unconfirmed`.
Unconfirmed does not prove absence or authorize a replacement charge. Received
retains the original expected EUR amount as a gross historical fact, including
after hold expiry or reordered open/failed observations. Later identity ambiguity
requires review and preserves earlier paid history. No net balance, refund,
invoice, stock, fulfilment or customer-facing authority follows.

Each immutable recognition binds its actual order, unique attempt, original money,
complete observation watermark, originally selected resolution receipt, derived
state and nondecreasing server timestamp. There is one canonical recognition per
`(order_id, through_observation)`, with watermark 0–1000. A separate current pointer
advances atomically with the journal and its minimal order outbox event. Missing,
malformed, noncontiguous or over-bound evidence fails closed. Readers revalidate
the exact original resolution, authenticated prefix, provider ownership and event
custody; storing another resolution for the same watermark does not invalidate
canonical replay. Historical lookup and same-prefix replay retain their original
receipt without fresh clock/identifier requirements or duplicate effects.
The recognition event binds a SHA-256 digest of the canonical PostgreSQL JSONB
object containing every journal field, as well as the order, attempt and recognition
identifiers. This retains the originally selected resolution and headers across
damaged imports; substituting another otherwise-valid resolution cannot silently
rewrite the receipt. The event carries no buyer/address or provider payload.

New recognition and current inspection explicitly establish READ COMMITTED before
any transaction read. They acquire the original attempt FOR UPDATE in a separate
statement, then inspect the complete current evidence and require a validated
resolution at exactly that watermark. Observation append uses the same attempt
lock, serializing evidence through commit. Native journal/pointer guards also
reject other isolation levels and independently lock before querying current
evidence. Their snapshot-sensitive functions are VOLATILE. The native proof
validates each bounded observation prefix in one ordered query, including
adjacent timestamps, the exact captured snapshot and its reciprocal event.
For refund history, one complete proof covers the longest referenced prefix
for the original attempt. Each shorter recognition still binds its exact
resolution, provider identity summary and event, and checks the observation
time at its own watermark.
The attempt lock remains a separate statement. New writes sample a
nondecreasing observed clock after proof/lock waits and again after actual event
and pointer writes. Failure or cancellation rolls back the whole decision.

A current pointer behind new observations is explicitly stale/unavailable; never
return its old state as current or synthesize unconfirmed. A successful current
inspection proves currency only within its locked transaction. A returned detached
receipt cannot authorize later effects. Future caller-owned operations must retain
READ COMMITTED and that same claim through their dependent transaction. Lock order
is attempt, recognition/pointer, then event; raw conflicting lock order may abort
under bounded lock/statement deadlines and must not reuse detached proof.

Native constraints independently derive state and original money, bind the exact
order/attempt/resolution/current prefix, preserve immutable history, prevent
pointer rewind or detachment, and require complete event/pointer custody at commit.
No route, worker command, scheduler, credential installation or provider I/O is
added by recognition. Sandbox/provider and deployed acceptance remain separate.

### Atomic settlement of original reservations

The private stock settlement consumes only the complete still-active original
reservation of an order whose unique attempt has current `received` recognition.
It creates one immutable successful settlement per actual order, binding the exact
attempt, original recognition receipt, original stock group, server-selected
`payment-settlement` actor, observed time and bounded source/component footprint.
It neither dispatches goods nor authorizes refunds, restocking or another charge.

The caller establishes READ COMMITTED before reads and retains one transaction
through commit. A crate-private recognition verifier claims the original attempt
in a separate statement and validates the current pointer, complete observation
prefix, original resolution, ownership and event/digest custody. Its returned data
is not detached authority. Unconfirmed, ambiguous, missing or stale recognition
cannot authorize new consumption. Canonical historical lookup precedes fresh
financial eligibility: replay validates its originally bound recognition and exact
immutable effects before requiring new clock or identifiers;
later payment ambiguity cannot repeat consumption or automatically restock goods.

Lock order is attempt/current financial proof, original sealed group SHARE, every
original hold UPDATE in database hold-ID order, then all union component balances
UPDATE in the same database component-code order as existing stock operations.
Expiry takes a hold before components and never the group. Do not acquire ordinary
active-reservation SHARE claims before these stronger preclaims, or introduce a
different collation for locks. Existing active-source validation and per-hold
transition helpers may run after all strong claims; effects follow the captured
member order. All physical components are claimed before the first consumption.

The manifest must exactly match the immutable order's group, owner/window, selected
SKUs, sealed BOM revisions and quantities. Bound reads to 20 members, 64 component
lines per BOM and 1,280 hold/component pairs or union components, with an explicit
one-MiB footprint limit. Cover and foam are kinds, not one-code-per-kind limits.
Multiple cover codes remain distinct physical components. Reject incomplete,
duplicate, excessive, changed or malformed source instead of truncating it.

The settlement receipt is inserted before effects. Its native VOLATILE INSERT
guard independently requires RC, fresh current received proof and the same ordered
claims. It validates complete original membership/BOM/reservation-movement custody,
all-active state, absence of any terminal movement for those active holds, and real
physical backing. It captures actual component before balances and expected after
balances from exact aggregate demand. The footprint also freezes each original
member's component quantities; an aggregate alone cannot preserve allocation.
This prevents adopting previously consumed groups or manufacturing completion from
valid-looking ledger rows that never changed physical balances.

Consume each member and append its existing canonical hold/component movements,
then the minimal real-order settlement event, in that same transaction. Native
deferred completeness proves every original hold consumed, exactly the required
reserve/consume movement set, original reserve bindings, fixed settlement actor/
reason/quantities/deltas on consumption and consistent
per-component intermediate after balances in captured member order. Actual final
balances must equal captured before minus aggregate demand for on-hand/reserved,
with quarantine unchanged. The immutable event binds all settlement fields and
the complete footprint with a canonical digest. Missing, extra, detached or
partially written effects abort the whole transaction. No caller-selected effect
key can create another settlement for the order.

Final validation also requires the bound recognition to remain current and
received. A caller can flush deferred constraints early, so successful final proof
acquires a transaction-level completion seal using a dedicated advisory-lock
namespace and a database-generated unique positive fence key. The key is immutable
and digest-bound; exhaustion fails closed. Existing locks never substitute for the
full proof. Native BEFORE guards reject subsequent related financial or stock
writes in that transaction, checking both old and new identifiers and movements
touching either original holds or affected components. Lookup starts with this
backend's granted exclusive two-integer advisory locks in the current database,
then indexed fence keys and bounded footprints; never scan all historical receipts.
Transaction seals cannot be manually unlocked and disappear at transaction end.
An early flush therefore prohibits later related writes, including overlapping
stock settlements in that transaction. Savepoint rollback must restore pending
proof if it releases a child seal; savepoint release retains the seal. Unrelated
writes and legitimate warehouse/financial changes in later transactions remain
permitted. A manually acquired matching advisory lock can only inhibit writes,
never authorize or skip proof.

After lock/proof waits and after all actual receipt/movement/event writes, sample
one nondecreasing observed server clock. A successful consumption remains strictly
before the original expiry and no earlier than its recognition. Expiry, rewind,
entropy failure or cancellation rolls back every effect. PostgreSQL ledger audit
timestamps are not inputs to the domain's explicit expiry clock.

Validate original source and complete reserve/terminal custody before classifying
an intact expired, released or mixed terminal reservation as `RecoveryRequired`.
An active hold with terminal movements, missing effects or invalid physical
balances is stored-data corruption, not an ordinary recovery outcome. A recovery
return changes no stock and does not consume canonical settlement identity. It
preserves durable gross paid recognition for separately designed late reallocation;
never reactivate expired holds or create fictional stock pools. A typed return is
not a durable operational review queue: activated processing must add that custody.

Historical readers revalidate the receipt's original order/group/recognition,
frozen footprint, exact immutable movements and digest-bound event. They must not
compare current warehouse balances with the old after footprint: later legitimate
stock operations change them. An order settlement event without its canonical
receipt is corruption. Receipt replay returns history without new effects.
No route, worker command, scheduler, provider call, runtime credential or deployment
is added by this primitive. Late-paid reallocation, durable review dispositions
and actual fulfilment remain separate requirements before operational acceptance.

### Late paid stock allocation and review

The next private stock path recovers only the frozen paid order's complete original
component demand. It never reprices the order, changes its money/attempt/address,
substitutes components or derives a new BOM from current catalogue selection.
No route, worker, credential, provider request, refund or dispatch is installed.

A single immutable allocation claim per actual order spans original settlement and
successful late allocation. It binds the unique original attempt, allocation kind
and exact owning receipt. Native reciprocal receipt/event custody rejects orphan,
changed or duplicate claims. Claim the order allocation before physical effects,
under the same READ COMMITTED attempt UPDATE claim used by financial recognition.
Backfill existing original settlements from their validated historical received
recognition, source/effects and digest-bound event; never require their old prefix
still to be current or rewrite their original receipt, event, timestamp or digest.
Both original and late readers must validate this shared allocation ownership.

Read validated canonical allocation or outstanding review history before fresh
clock/entropy, then repeat after obtaining the original attempt claim. An invalid
claim or contradictory history fails closed, without falling through to another
allocation. New work requires current exact `received` recognition retained in the
same transaction. Its full prefix, resolution, ownership and event proof remain
locked through commit; detached payment data grants no stock authority.

Revalidate the complete original order/group, sealed BOM and reserve/terminal
movement custody before classification. Retain every member's actual state.
`RecoveryRequired` from original settlement is a broad return, never admission
permission. An original containing any consumed member without canonical
allocation requires durable `prior_consumption_without_allocation` review. Any
released member requires `release_intent_unresolved` review: generic release
history cannot distinguish cancellation from permission to allocate again.
Corruption is `InvalidStoredData`, never a normal review or scarcity outcome.

Automatic late allocation admits only intact ACTIVE/EXPIRED originals at or after
their original deadline, with no consumed or released member. All-active originals
still within their window are not applicable to this path. New time must be no
earlier than the order or bound recognition and must remain nondecreasing.

Retain lock order: attempt, original sealed group SHARE, all original holds UPDATE
in database hold-ID order, fresh group/building children, then every union balance
UPDATE in the existing database component-code order. No preliminary hold
SHARE-to-UPDATE upgrades or alternative collation are permitted. Fresh group and
child IDs are generated server-side and inserted fresh; collision cannot adopt a
legacy group through reservation replay. Freeze the planned fresh membership/BOM
before effects and require complete sealed membership after reservation.

For each component, before balance `(P,Q,R)`, full frozen demand `D` and original
active demand `A`, require valid stock, `R >= A` and `P - Q - R + A >= D` using
checked integer arithmetic. Claim and check all components before changing any.
Missing or invalid balances are corruption. Other orders' reservations remain
untouched; cover/foam are kinds and multiple physical codes remain separate.

Insert the successful receipt and canonical allocation claim before effects.
Expire only remaining active original members, then reserve the full demand on
fresh children and consume every fresh child, all in the same transaction.
Already expired members get no repeated effect; original identities and windows
never reactivate. The fresh server window is exactly 60 seconds, within the
existing hold policy, and is an engineering bound rather than a customer promise.
Prepare the fresh window before component waits; `expires_at` is checked
`prepared_at + 60`. Sample `allocated_at` after all claims and require
`prepared_at <= allocated_at < expires_at`, with final time still before expiry.
Waiting cannot extend the window. Final balances must be exactly
`(P - D,Q,R - A)` with quarantine unchanged.
Reservation retains its existing order-derived audit actor; expiry and consumption
use fixed server actors validated by their native proofs, never caller claims.

Factor narrow fresh-manifest preparation and claimed reservation phases from the
existing basket primitive. Preserve its public replay/transaction behavior.
All fresh building children precede component claims; reservation then publishes
exact hold lines/members and seals the group. Do not duplicate basket policy or
acquire contended original/fresh hold claims after component effects.

Fresh construction, any pending allocation/receipt and its component claims live
inside a savepoint; retain attempt/original group/hold claims outside it. If stock
is scarce, roll back that savepoint so no fresh rows, allocation or physical effects
remain. This also releases its component locks. Reclaim balances in database order
and refresh financial/source/time proof. Persist `component_scarcity` review only
if the new locked snapshot is still scarce. If availability changed, return a
retryable `StateChanged`/unavailable result without a case or reconstruction loop.
Never reset the observed clock across rollback; entropy/sequence use is not a
physical business effect.

A successful bounded footprint binds original states and exact source movements,
fresh identities/window/planned membership, aggregate active/full demand and each
component's real before, expiry, reserve and consume stages. Bounds remain
20 members and 64 BOM lines per group, up to 2,560 original/fresh pairs and 1,280
union component codes. Read at most the complete 5,120 reserve/terminal movements
plus a sentinel. The late/review encoded envelope is capped at two MiB; test the
largest closed source fixture and reject oversize before JSON/digest evaluation,
including the entire event/digest subquery. No truncation or guessed two-code limit.

Native BEFORE and deferred completeness independently require RC, current received
proof, the common claim, complete original/fresh binding, exact stage movement sets
and member-order intermediate balances, real final parity and the digest-bound
order event. Each stage projects the bounded component footprint once and joins
it by component code before verifying the ordered movement ledger. Sample final
observed time after all actual writes and reject rewind
or fresh-window expiry with full rollback. Early constraint flush acquires a
transaction completion seal only after the full proof. Extend scoped mutation
guards across original/fresh groups and holds, affected components and financial
sources. A manually acquired advisory lock cannot skip proof; savepoint rollback
must restore pending proof while releasing its child seal. Later transactions may
perform legitimate financial or warehouse work.

Review cases are immutable initial stops, not physical allocation claims. Bind the
actual order/attempt, original historical received receipt, closed reason,
assessment time, bounded source footprint and a digest-bound minimal order event.
Review admission proves that snapshot under current source/financial claims; its
commit proof requires immutable case/event custody. It deliberately has no
completion seal or guarantee of current scarcity after later same-transaction
changes. Successful original/late allocations retain the full final-current proof
and seal above. Native guards serialize cases and allocations on the original
attempt. A recorded stop blocks automatic retry after restock, process restart or
a new caller key. Only the explicitly confirmed scarcity command below can link
a later allocation to an existing case. Finance reconciliation uses a separate
capability. Installation seeds no stock recovery grants.

A private staff inbox requires the dedicated persisted `stock.review` grant, an
enabled session and recent MFA, rechecked after reading while authority rows remain
locked. No grants, login or HTTP route are installed. Each live C-collated keyset
page scans at most four candidate case IDs and validates each complete historical
custody proof before applying an optional closed reason filter. Invalid custody
fails the entire page. The minimal summary exposes the case/order IDs, assessment
time, closed reason, exact case digest and validated retry status. A cursor advances
across scanned candidates even on an empty filtered page. This is not a snapshot: inserts at or before the cursor require refreshing
from the beginning. The read grants no retry, disposition or financial/stock effect.

Historical review validates captured immutable identities and exact per-pair
reserve/terminal sequence references and semantic-row digests, their closed
captured states and original recognition. Digests cover identity, actor, reason,
quantities and balance effects; audit timestamps must not make them depend on
session time zone.
It is an admission-validated assessment, not proof of current availability or
permission. A captured active member may later have a valid terminal transition;
claim original holds SHARE in database order before coherent multi-query history
reads. Do not require captured states to equal current states, use a global stock
sequence as commit chronology, or compare current balances with old snapshots.
Captured terminal references must remain exact. Successful history validates all
original/fresh effects and claim custody without requiring current recognition or
warehouse balances. Automatic historical replay uses the existing receipt without
new time, IDs or effects.

### Staff-authorized scarcity retry

One private command can retry an exact historical `component_scarcity` case.
Require the separate unseeded `stock.recover` capability, recent MFA and explicit
confirmation of the case ID, immutable case digest, expected next generation
and a bounded operator reason. The staff session supplies the actor. The
32-character lower-case hexadecimal effect key binds the complete intent.
Allow at most three contiguous committed generations per case. A fresh
confirmation is required for each generation. Replayed exact intent can return
the validated historical result under current staff authority. Conflicting
reuse of an effect key or a stale generation cannot start another attempt.

Claim staff authority first and keep it locked through commit. Claim the
common payment attempt before inspecting current state or deriving a new
generation. Revalidate the original review source and event, current received
recognition for the same order and attempt, the original frozen order and
complete hold custody, and the absence of any existing canonical allocation.
An originally released or consumed case stays stopped. If an eligible
scarcity case's current holds have since been released or consumed, deny a
retry. Use the same stock equations, claim order, fresh 60-second window and
completion seal as the automatic path. Check the observed clock and staff
authority again after writes and waits. A transient failure, cancellation or
state change rolls back the decision and all attempted effects.

A committed decision has either one exact successful allocation or a fresh
still-scarce assessment. Before a successful retry makes stock effects, insert
the immutable decision in the same savepoint as its exact late allocation,
using a pre-generated receipt ID. The common allocation claim remains unique.
Keep the original review case and its event. Native guards allow review and
allocation to coexist only through the exact authorized successful decision.
Deferred validation requires the reciprocal decision, allocation, canonical
claim and digest-bound events. Preserve native final physical and current
financial proof even if constraints flush early. At admission the decision
binds an authenticated staff account through Rust; native row custody and its
actor foreign key do not authenticate an arbitrary database writer.

If the initial fresh attempt finds scarcity, roll back the savepoint and
reclaim the source and stock before recording the current locked assessment.
Bind its bounded footprint and refreshed financial proof to one still-scarce
decision and event. Retain the original case and leave stock intact. If the
fresh snapshot is no longer scarce, return a state-change error and require
another confirmed attempt. The assessment is historical after commit.

Keep historical reads bounded and fail closed on any detached decision, event,
case or allocation. An allocated case can appear as resolved only after its
reciprocal evidence validates. A still-scarce or never-retried case remains
open. No scheduler or automatic retry derives authority from the case or its
event. The private command has no provider, refund, invoice, shipment, or
public-route effects.

### Supervised test-payment recovery

The explicit local/test recovery command uses a separate authenticated GET-only
client: its verified type exposes no create operation. A fixed Mollie API origin,
test-key capture and expected-profile fetch protect every bounded list/payment GET.
A dedicated test-profile key must be supplied explicitly; installation and normal
worker probes create no credential or provider activity. Rotated same-profile test
keys remain recorded in immutable observation/search receipts.

One durable job per original dispatch consumes the exact immutable recovery outbox
binding, scoped to the verified profile. Search and candidate checkpoints survive
process loss. A stable page-operation receipt is read before a list retry; fetched
facts use a fresh operation per lease so a delayed worker cannot replace a newer
status or erase paid evidence. Every candidate requires its own authenticated GET.
The full page chain is inspected before polling a single identity; ambiguity always
stops for review. Empty/end/capped scans remain unresolved and never grant a POST.

Read-committed SKIP LOCKED claims carry a 60-second lease and increasing generation.
No database transaction crosses provider I/O. Progress updates require the exact
unexpired generation/lease and final nondecreasing server time after the write.
A stale worker may preserve authenticated evidence but cannot advance the queue.
Cancellation or checkpoint failure leaves immutable receipts available to recovery.
The invocation performs at most 1–25 steps (default 10); jobs retain exact page,
candidate, operation, poll and failure positions. Native guards preserve source
custody, bounded counters, page ownership and irreversible review stops.

Transient provider failures back off for 60/120/240/480 seconds, then stop after
five failures. A single unresolved-status identity polls at 60-second intervals,
up to ten times. Scans stop at 100 pages and jobs at 1,500 claims. Paid evidence,
ambiguity, unresolved/capped scans, exhausted budgets or invalid custody create
an explicit needs-review reason; they are never silently restarted. Paid evidence
still requires the separate financial/late-stock recovery slice. The command emits
counts/static diagnostics only. Final profile-scoped outstanding review/backoff
counts include unchanged earlier jobs, so later passes keep returning failure
until those conditions are resolved through authorized recovery.
No service scheduler, runtime key, callback route, current payment/fulfilment/refund
transition or production supervision is installed by this implementation.

### Authorized transient recovery

The private `resume_transient` entry requires the unseeded `payment.reconcile`
finance capability, an enabled persisted session, recent MFA, confirmation,
expected stopped generation/consumed-resume revision, exact original profile and an incident reason.
Only exhausted transient GET failures may resume, at most three times over the
attempt's lifetime. Claim/operation counters, page/candidate checkpoints and
poll/scan limits remain intact. A retained page resumes candidate processing;
no page resumes discovery. Every provider operation remains GET-only. Other
review reasons require separate dispositions and stay stopped.

The staff authority lock spans commit. Immutable per-attempt sequential decisions
retain actor, reason, full before/after checkpoints and a global intent-bound
effect key. Native guards permit only the exact audited transition and require
its checkpoint update in the same transaction through a deferred constraint. A
monotonic consumed-resume revision advances once per decision; native guards forbid
rewinding it or reusing a historical receipt after reconstructing a stopped state.
Final nondecreasing time and session/MFA checks follow the actual writes. Historical
retries return the original decision without restarting later progress, while
still requiring fresh authority. No public route, staff grant, provider key,
financial disposition or operational activation is installed.

### Callback-ID fetch and original custody

Legacy payment `webhookUrl` notifications carry an untrusted form `id`, which must
trigger an authenticated GET; they supply no payment-status authority. This is
separate from next-generation snapshot webhooks. The owning provider contract is
[Mollie's legacy webhook reference](https://docs.mollie.com/reference/webhooks).

The verified GET-only test client can now fetch a bounded provider ID without a
previously known attempt. Canonical wire decoding preserves duplicate critical
field rejection, closed Test/EUR types, exact decimal amounts and one-off identity.
Resource, expected ID/profile, metadata schema and bounded original identifiers
must match before an opaque callback result is returned. Authenticated metadata is
only a lookup hint. The private recorder restores the immutable original
order/attempt/dispatch, performs complete financial matching and repeats that
matching under the existing serialized observation append. Late/reordered facts,
exact historical operation receipts, actual fetched-key fingerprints and the
minimal atomic processing event retain their existing semantics. Callback evidence
discards optional checkout capabilities and provider/customer details.

Provider response recording starts only after I/O completes. No caller-controlled URL,
callback amount/status, client body or financial transition enters this boundary.
The private durable lookup boundary now commits an immutable server-generated
operation with the expected profile and provider ID before I/O. Only its opaque
committed capability can obtain a durable fetch start and a single-use GET permit
consumed by the transport. Recovery reads the committed receipt before doing I/O. A started
operation without a receipt remains visibly unresolved; it cannot issue another
GET. A fresh fetch requires a new operation. Concurrent writers serialize on the
operation under read committed isolation and retain one full-intent receipt rather
than replacing earlier evidence. A future bounded inbox supervisor must retain
each generation's operation so late facts from an earlier GET remain recordable.

Each bounded minimal fetched receipt commits with a dedicated callback processing
event. This aggregate is independent of orders: `commerce_outbox.aggregate_id`
requires a real checkout order, and an authenticated unknown attempt must remain
visible without inventing one. The callback event records custody for later
processing; it is not an order/payment transition. Immutable database bindings
and deferred completeness checks protect the receipt/event pair. The event binds
the SHA-256 digest of the complete canonical PostgreSQL JSONB snapshot; a valid
status, amount or fingerprint changed in a damaged import cannot retain the
original event's custody. Trusted reads
bound stored data and revalidate closed snapshots, headers and the exact event
before reconstructing the private fetched capability. Saved JSON alone cannot
enter the observation recorder.

Processing uses a namespaced operation effect and restores the complete original
order/attempt/dispatch before the existing serialized observation/event append.
The durable lookup start must not precede the original dispatch; installing a
later dispatch cannot authorize an older unmatched provider fact.
An unknown attempt or mismatched original fails processing while retaining the
callback receipt. A crash after observation commit can replay the same operation
without duplicating that observation. Late/reordered facts and actual fetched-key
fingerprints retain their existing semantics. No transaction spans provider I/O.
This boundary installs no receiver, public route, bounded inbox supervisor,
credential, scheduler, payment/stock/fulfilment action or production activation.

### Private callback inbox and supervision

Callback intake has independent immutable source events and jobs. It cannot use
order-bound reconciliation jobs or the receipt-only callback processing events
as pre-fetch custody. Admission retains only a server-generated ID, configured
profile, untrusted provider ID and time; it neither invents an order nor stores
incoming status, amounts or raw bodies. Repeated provider IDs remain admissible
because later notifications may represent changed state.

The local/test policy caps active intakes at 200 globally and 50 per profile,
with 120 global and 30 profile admissions per 60-second window. Native quota locks
serialize admission and reject clock rewind; intake, event, job and quota effects
commit together. These are private engineering limits, not measured production
capacity. An independent indexed active-custody set bounds capacity and supervision
reads; a mutable settlement flag alone cannot hide work. Its minimal headers are
audited globally within the 200-intake bound before profile selection. A job
releases capacity only when its current work is done and every created generation
has a successful, non-review receipt disposition. An older unresolved GET or paid,
ambiguous, unmatched or invalid-custody disposition retains capacity and visibility.

Each claim atomically commits a fresh generation, exact lookup-operation binding,
single-use fetch start and 60-second lease before yielding a usable GET permit.
An immutable consumed-claim receipt prevents reusing a historical lease after a
damaged import recreates an older mutable checkpoint.
A batch claims at most 25 steps; an intake has at most five lifetime GET
generations. Transient failures back off from 60 seconds and end in visible
review. Committed receipts are recovered before another GET, including when a
bounded processing backlog has not yet reached them. No transaction spans I/O,
and lease expiry cannot grant a replacement payment create.

Receipt processing drains independently of the latest job checkpoint, including
after a newer open result or a review stop. A delayed worker may retain its
authenticated result under the original operation; it cannot advance a newer
checkpoint. Complete lookup digest/start/original matching precedes observation
and sticky identity resolution. Immutable dispositions bind that exact custody;
historical reads revalidate it instead of trusting a saved outcome string.
Recovery checks the saved receipt's time before any fresh observation or identity
effect, so clock rewind cannot poison later retries with earlier immutable facts.
Malformed receipt or deterministic original-history failures retain a source-bound,
redacted exception without reconstructing payment evidence, and processing can
continue for other receipts. If proof behind an already committed disposition
is damaged, a separate immutable, source-bound quarantine preserves that original
row and its facts. It retains admission capacity and a visible review stop while
unrelated work continues; repairing the proof does not silently remove quarantine.
A paid
or ambiguous fact remains an operational exception, not fulfilment authority.

The explicit local/test worker command installs no key or scheduler. It reports
outstanding review and retry work even on unchanged later passes. Public form
parsing, trusted-edge abuse controls and an exact staging callback exception
remain separate work before public notification acceptance. Production remains
disabled; no financial, stock or fulfilment transition follows from this inbox.

### Private classic callback form intake

The private receiver uses `POST /api/callbacks/mollie`, behind the existing
preview Bearer gate and loopback-only API. It is absent unless a validated
`CALLBACK_INTAKE_PROFILE_ID` is configured; production startup remains rejected.
The profile comes only from server configuration. Intake needs no provider key,
performs no provider request and grants no payment, stock or fulfilment authority.

Local closed-input policy is a maximum 1024 raw body bytes, one Content-Type
header of at most 128 bytes, `application/x-www-form-urlencoded` with at most one
optional UTF-8 charset parameter (case-insensitive, optionally double-quoted),
and no Content-Encoding. After strict percent/UTF-8 decoding there must be exactly
one field named `id`; duplicate decoded names, unknown fields and empty segments
fail. The value must satisfy the existing provider-ID policy, including its
256-byte bound. These limits and rejection rules are local policy, rather than
additional provider guarantees. Raw bodies and identifiers never enter logs.

Return the same empty `200 OK` for every committed intake, including unknown IDs.
Commit the intake, source event, job and active custody before acknowledging.
Malformed or unsupported input has no database effect; storage, quota and timeout
failures cannot produce a success acknowledgement. Request cancellation may leave
no transaction or an already durable intake; a later notification remains
admissible because classic callbacks have no stable delivery identifier.
The worker fetches and verifies payment facts independently.

This follows the [classic webhook contract](https://docs.mollie.com/reference/webhooks)
and [payment-ID contract](https://docs.mollie.com/reference/get-payment).
Next-gen JSON/signature events are a separate protocol. This private adapter does
not add a public staging POST exception, trusted-edge policy or deployment.

### Disabled staging callback edge

A separate Rust edge binary in the API crate forwards only the classic callback
POST to the private loopback API. This preserves Next's browser POST gate and
provides actual socket-peer metadata. It has no database credentials, provider
key, payment fetch or financial authority. The ordinary API remains loopback-only.

`CALLBACK_EDGE_ENABLED` is absent/false by default. Production is rejected even
when disabled; enabled local/test/staging configuration requires a validated bind,
loopback `API_URL`, server `PREVIEW_TOKEN` and one to four distinct exact numeric
`CALLBACK_EDGE_PROXY_IPS`. Staging accepts only private proxy addresses, requires
the exact source revision and binds the dedicated container port 45672. Local/test
may use loopback peers and ephemeral ports. No subnet, gateway assumption, Host,
XFF or other incoming header establishes trust. The peer ACL is a network boundary,
not cryptographic proxy identity or provider authentication.

Check the actual connection peer before consuming a body. Admit at most 16
concurrent callback requests globally; read at most 1024 body bytes within one
second, reuse the closed classic-form/media policy, and finish each request within
three seconds. Forward raw validated bytes to the fixed `/api/callbacks/mollie`
path with newly constructed canonical form Content-Type and server Bearer headers.
Do not forward caller credentials, cookies, Host, Origin or forwarding fields.
The infrastructure client disables environment proxies, redirects and automatic
POST retries, and bounds the complete upstream exchange to 1500 ms. Only a finished
empty upstream 200 permits an empty external 200. Other upstream results are static
non-success responses; no body or diagnostic header is relayed. Cancellation may
leave no admission or an already durable intake; it cannot grant a false success.

The base staging Compose file stays unchanged. A separate optional overlay/profile
defines the edge image, API profile configuration and edge-owned Traefik labels.
`STAGING_CALLBACK_INGRESS_ENABLED` defaults false, independently of starting the
private edge process. The only HTTPS router matches the exact host, path and POST
method with explicit priority/service. There are no credentials in labels. Use
per-remote-address rate limits, an overwritten nonsensitive constant inflight group,
global inflight limits and 1024-byte bounded buffering without buffering retries.
Existing web routing and its Basic gate remain intact.

Source checks do not establish the existing proxy's version, unique peer mapping,
address stability or no-bypass network boundary. Public activation also requires
verified nonzero ingress read timeouts: Traefik buffers uploads before Rust's body
deadline begins. Confirm the actual middleware behavior, exact-revision TLS and
provider delivery in a separate authorized isolated pilot. Do not trust a shared
SNAT gateway or the whole Coolify subnet as proxy identity. No public callback
ingress, deployment or production activation follows from adding this source.

## Sessions, uploads and rendering

Rust owns secure sessions and server-side authorization. Use a maintained password/session implementation with Argon2id where passwords are stored; verified email, rotating sessions, hashed reset tokens, admin MFA and separate guest-order access tokens. Browser credentials are HttpOnly/Secure/SameSite cookies; never store bearer tokens in browser local storage. CSRF and origin checks protect cookie-authenticated mutations. No duplicate identity system in Next.js.

P2 staff capability guards consume server-side account, session and grant records.
Installation creates no accounts, grants or sessions; accounts default disabled.
Opaque 256-bit session tokens are stored only as SHA-256 digests and never implement
Debug/Serialize. Catalogue/BOM inspection requires `catalogue.manage`; warehouse
mutations require `warehouse.manage` and explicit confirmation. The public warehouse
command has no actor field: the session supplies the ledger actor. No owner/all-role shortcut exists.

The guard locks the account, session and grant with `FOR SHARE` in the same read
committed transaction as the operation, blocking disabling, revocation and grant
deletion through commit. Clock capture happens after lock acquisition, so waiting
cannot reuse a timestamp from before session or recent-MFA expiry. A second check
before commit/return rejects expiry or clock failure during inventory/data waits,
rolling back pending stock effects. Sessions last
at most 12 hours; MFA evidence must be present, not future-dated and at most 900
seconds old. Role, clock and database failures leave no stock effect. Idempotent
receipt reads require the same live authority checks. These are scoped Rust
operations with no new HTTP route. P4 must supply genuine login/MFA verification,
session issuance, cookie/CSRF controls and revocation endpoints before admin use;
the current tests use labelled session attestations in disposable databases.

Public catalogue HTML is cacheable by explicit locale/product version. Private cart, account, admin and quote data are never shared-cache responses. Invalidation follows publication/price/stock changes. Public facts and JSON-LD come from the same approved catalogue projection.

Room planning starts with local dimensions and a calibrated 2D footprint. A photo overlay is an explicitly approximate visual aid. Camera AR is deferred until measured product assets/device support and accessibility fallback are proven. Default room images stay on-device; opt-in persistence uses private storage, quotas, expiry and deletion.

Approved-catalogue preview reads use an explicit customer-field projection of the
current activation and revalidated immutable sources. Variant claims precede all
ordered asset claims; no previous activation, missing locale or draft is a
fallback. Stock availability uses one statement over the sealed physical BOM and
available balances; an unreceived component has zero availability. Audit/source/
qualification references and warehouse counts never enter the HTTP DTO.

The preview HTTP collection, exact-SKU detail and revision-bound image routes
retain bearer authentication, bounded requests, noindex and private/no-store.
They do not enable production. Image delivery accepts a SKU/locale/publication/
edge, checks eligibility, loads only its sanitized derivative, ends the transaction
before bounded CPU verification, then rechecks current eligibility and the exact
image identity before returning bytes. No original/asset-path route is exposed.

The private collection and family/variant HTML use the same validated projection
as the preview XML feed. Each feed/collection traversal is bounded and fails as a
whole on dependency/cursor errors; no missing locale or selected SKU falls back.
Language switching preserves the SKU and resolves its current locale family path.
Ordinary variants share the configured family canonical; draft previews emit no
indexable alternates or merchant Offer schema. Public selling feeds/schema remain
P6/P0 acceptance work. The specimen stays a separate clearly labelled route.

Approved collection routes await their data before the response starts. The shared
loading boundary is scoped to specimen products/gallery so an unavailable SKU can
return a genuine 404. Every Next/API/image request remains private/no-store. Image
proxy paths accept only locale/SKU/revision/edge and bound response bytes while
reading; private credentials and original/asset paths never reach browser props.

### Guest basket sessions

The optional internal guest routes remain behind the preview bearer and loopback
binding. `GUEST_ORIGIN` must be a canonical HTTPS origin. Rust's maintained
`tower-sessions` transport issues `__Host-piwas-guest` cookies with Secure,
HttpOnly, SameSite=Lax, path `/` and no Domain, including removal cookies. Lax
permits a later provider GET return; a return grants no payment authority. There
is no Next.js identity copy or browser storage bearer. Installation creates no
credentials or baskets. Staging configuration does not enable these routes yet.

Each opaque 128-bit framework identifier is stored only as a domain-separated
SHA-256 verifier. Its immutable owner and random 256-bit synchronizer CSRF value
remain server-side with a fixed maximum seven-day lifetime. The owner-only,
no-store response supplies CSRF; mutations require one exact configured Origin
and one matching CSRF header. Issuance requires that Origin and closed empty JSON.
The API bounds bodies at 4096 bytes, rejects positional JSON shapes and accepts
only SKU/unit intent. Host/forwarded headers cannot select the trusted origin.
Cookies and CSRF/body values are excluded from logs. Raw framework errors are
mapped to static codes; a failed create clears its pending middleware state.

The PostgreSQL store never upserts a credential, changes its binding, extends its
lifetime or resurrects revocation. Fresh principal/identifier collisions fail the
whole transaction. Operations hold credential SHARE claims; revocation takes an
exclusive claim, and fresh clocks reject expiry or rewind during waits. Basket
versions and immutable full-intent receipts serialize updates; historical retries
return their old revision without reverting newer selections. Atomic issuance
and per-basket mutation minute ceilings are resource bounds, not per-IP bot
protection. Public rollout still needs an explicit trusted-edge abuse policy.
Guest identity grants no staff, catalogue, stock, price or order authority. Basket
checkout must freshly validate publications, market policies and aggregate stock,
then create immutable order snapshots and whole-basket holds. Account linking
and privilege changes require separately proven credential rotation in P4.

### Owned indicative quote snapshots

The optional private guest API persists immutable version-1 EUR quote receipts.
A mutation claims the active credential and basket before current revision and
closed destination validation. Client input contains only expected basket revision,
effect key, country and postcode. Existing exact effects return their original
unexpired receipt without rewriting the current cart, charging issuance quota or
extending expiry. Changed revision, locale or destination conflicts. New issuance
requires the current nonempty cart and latest independently approved exact market
policy, eligible publications/BOMs and sufficient aggregate physical stock.

The read-committed transaction retains policy, publication, asset and stock claims
through snapshot insertion. A shared observed clock rejects any backward sample
across nested readers. One final sample after writes checks identity, policy,
photo permissions and the captured deadline before commit. Cancellation or failure
rolls back both the quote and its per-basket ten-successful-issuances/minute quota.
These counters supplement existing resource bounds; public edge abuse controls
remain required. A fixed lifetime is at most 300 seconds and never a price promise.

Snapshot rows bind the immutable basket receipt, internal locale, destination,
policy draft/review, publication/BOM references and checked price/net/VAT/shipping
allocations. SQL guards enforce immutable history and exact relational bindings;
read adapters bound stored JSON, require closed object shapes and reconstruct all
money allocations. Server ownership claims protect issue, replay and read paths.
HTTP projections omit owner, destination and internal policy/evidence references,
map internal `kmr` to public `ku`, and remain private/no-store behind preview auth.
Installation seeds no quote, readiness, guest identity or commercial authority.

A private server-selected promotion can be captured in a version-2 indicative
quote. The outer snapshot preserves the exact promotion draft and independent
approval, allocation version, per-SKU discount and discounted gross/net/VAT,
unchanged shipping, and totals. Its nested version-1 snapshot retains the
original amounts and owned basket, market and catalogue revisions. The promoted
format shares the quote identity, effect-key namespace and per-basket issuance
quota with existing quotes. A guest cannot satisfy a per-customer limit.

Private verified quote issuance holds the verified account and current customer
session before claiming the guest session and basket in the same transaction.
An immutable relational field binds the verified account to the existing quote
format. The same account and guest ownership are required to read or replay it.
The shared quote effect namespace and issuance quota still apply. An approved
per-customer term can be captured here. Guest checkout rejects an account-bound
quote. Verified order acceptance retains the same account and basket claims
through the order and debit commit.

For a new promoted quote, claim the basket, reviewed market and promotion
anchors, then the sorted catalogue and physical component sources. Keep those
claims through insertion and the final clock and ownership checks. Cap the
quote expiry at the promotion end. Exact replay and reads validate the stored
allocation against the original immutable approved source; later withdrawal does
not extend or confer authority on an unexpired historical receipt. The legacy
quote projection rejects the promoted format. Private guest order acceptance requires
the original commercial sources and the exact current approved promotion and
allocations again before committing the debit.

An unexpired historical snapshot can remain readable after source withdrawal; this
is owned history, not fresh readiness. Order creation must independently compare
current basket and commercial terms, claim current reviewed sources, reserve the
whole basket and persist order/address/policy/promotion/outbox effects atomically.
It must acquire stock UPDATE claims directly in the established global order;
calling the indicative SHARE-stock evaluator first would risk lock upgrades.
No browser checkout, order, payment or deployment is enabled by these receipts.

### Private accepted orders

A private verified promoted order requires the account bound to its owned
quote and a current session for that account. The transaction claims account,
session and basket before market, promotion, catalogue and physical stock.
Each attributed ledger row retains the lifetime redemption sequence and the
same account's cumulative count. The promotion lock serializes both global
and per-account limits across policy revisions. A capped policy fails closed
if earlier debits for that promotion have no account attribution. Capacity stays
used after expiry or cancellation. Guest payment preparation, attempt history
and guest order-link proofs reject account-bound orders.

The infrastructure-only order primitive composes fresh guest/CSRF authority,
current basket revision, an owned unexpired quote and the latest independently
reviewed market/publication sources. It acquires globally ordered component
UPDATE claims directly, compares exact quote money/source revisions and requires
new acceptance after changes. The original quote is retained rather than silently
repriced. One quote creates one order; full-intent retries return the original
receipt before current cart/quote checks, without extending holds or altering a
newer basket. A different effect for an already-ordered quote conflicts; no
successful alternate key is acknowledged without a durable full-intent binding. Ownership and fresh session validity still protect every replay/read.

One database transaction creates immutable buyer/shipping/billing and explicit
seller/legal/tax document snapshots, full market policy, merchandise selection,
publication/BOM references, exact EUR allocations, sealed whole-basket holds and a
minimal `checkout.order_created.v1` outbox event containing only the order ID.
Final observed-clock checks follow these writes and recheck identity, retained
source/photo rights, quote, seller policy and hold window. History remains readable
after indicative deadlines; a history read grants no stock or payment transition.
SQL guards enforce exact owned quote/manifest bindings, immutable rows and the
required event. Stored readers bound JSON, require object shapes, validate headers
and reconstruct captured allocations against immutable source rows. Private
contact/seller/order types have no Debug implementation; database error details
are discarded at this boundary.

Private pre-dispatch cancellation requires the owned order, guest CSRF and a
current session for its account when bound. Cancellation and first payment
attempt insertion serialize on the same basket row. With an admitted attempt,
cancellation holds that basket then the attempt row, while dispatch holds the
attempt row. Read-committed native guards check after the claim so neither can
commit against the other's winning effect. The receipt binds the exact admitted
attempt, if present, and effect key, commits with a minimal outbox event, and
rechecks current authority after the writes. Historical pre-attempt receipts retain
their original meaning. Existing stock expiry handles the reservation, and the
original redemption remains part of lifetime usage. A durable dispatch start
requires separate reconciliation and disposition.

Seller policy is a required trusted server argument with explicit identity, tax
registration choice, document versions/digests, locale, evidence and validity.
Nothing installs or infers real seller/legal facts. Email validation checks bounded
single-mailbox syntax only, including UTF-8; it proves neither delivery nor identity.
Initial orders require a positive payable total. Version 1 orders retain their
original snapshot format. Version 2 orders retain the exact promoted envelope.
Payment attempts, recognition, refund limits and linked-order amounts use the
captured payable allocations. Stock settlement uses original quantities and BOMs.
Order, payment and fulfilment transitions, retained-data operations, public
HTTP/UI/edge controls, outbox processing and actual provider transport remain
separate work. No transaction performs provider/network I/O or sends a customer
message.

### Private verified customer identity

Private issue-for-delivery claims the existing email-verification quota once.
It reserves a nonce against a separate versioned key and the exact verifier in
an independently committed transaction. AES-256-GCM seals the bearer before
the challenge transaction. Authenticated data binds the purpose, normalized
identity, verifier, creation and expiry, envelope version, key and nonce.
The challenge and ciphertext commit together under reciprocal native guards,
with exact readback and a final observed-clock expiry check. The receipt omits
the bearer and the envelope stores no duplicate mailbox. Legacy direct issuance
retains its private trusted-caller contract.

A private delivery start locks the challenge before reading the exact stored
envelope, nonce and registered key fingerprint. The immutable start has one row
per verifier and requires an earlier envelope transaction, a live challenge and
durable commit. Consumption serializes on the same row, and native checks
prevent starting and consuming in one transaction. The standalone claim performs
no decryption or network I/O. A committed start is local custody with no provider
outcome, and a repeat cannot create another start. A distinct trusted in-process
path can authenticate and open only the envelope whose start it just committed.
It re-locks the challenge, revalidates the start and source, authenticates the
ciphertext and matches the recovered verifier. It releases the transaction before
passing short-lived borrowed values to its caller. A prior start cannot be used to
reconstruct the in-process authority.

A separate immutable outcome can retain only a closed accepted, explicit rejection
or uncertain classification, observation time and exact prior start transaction.
Only the fresh-opening in-process path can record it. The callback runs without a
database transaction. A later durable transaction matches the captured start.
Neither expiry nor redemption invalidates a historical result. A start without a
result is unknown and grants no resend authority. The database records the trusted
caller's classification. Provider authentication, mailbox receipt and redemption
each require their own evidence. No provider transport or retry is installed.

A private PostgreSQL operation locks and consumes an exact email challenge while
inserting a unique immutable customer identity and its verification reference in
one transaction. A database-assigned transaction ID on the consumption must match
the account insert transaction; legacy consumed challenges cannot be adopted. This
database check attests transaction custody, while Rust verifies token possession;
the database write role is trusted and must not be exposed to untrusted clients.
The final freshness check follows the insert and any uniqueness wait. The public standalone challenge consumer commits independently and cannot
be used to establish an account later. No password, session, guest-order link,
delivery or production authority follows from this identity record. A default-off
test-only loopback HTTP consumer invokes this operation once after preview auth and
exact configured Origin validation. Its dedicated admission key is configuration,
never request authority; the response carries no account identifier or session.
Timeouts do not establish whether the account transaction committed.

### Private customer password credentials

The verified mailbox may receive a separately scoped password-change proof.
Its random bearer verifier binds the account and the current credential
revision, including revision zero before setup. The proof expires after 15
minutes. Use the exact existing mailbox normalization for both issuance and
replacement. The trusted caller receives the raw bearer after the challenge
commits.

Validate the proof before starting bounded Argon2id work outside the transaction.
The replacement transaction claims the account before the challenge, rechecks
the revision and time, and commits the consumed proof with the new current
password hash. A database-assigned transaction identity and reciprocal deferred
check bind both writes. The caller flushes that check before the final clock
check. Historical challenges retain their consumed status, with only the current
hash in the credential table.

Replacement requires a trusted immutable offline blocklist snapshot with a bounded
validity window and integrity digest. After the proof precheck and outside any
database transaction, it checks the exact SHA-256 of the NFC-normalized password
before Argon2. A final clock sample rechecks validity before commit. The snapshot
is not a live revocation mechanism and does not prove corpus or reviewer quality;
corpus approval and coordinated distribution remain operational work.

### Private customer sessions

Private password login reads the bounded current credential, releases the
database transaction and verifies it using the same bounded Argon2id worker
as replacement. An absent credential gets a dummy verification for valid
password input. Before issuing authority, claim the account and recheck the
exact hash and revision. Store only a domain-separated verifier for the
random bearer and return the bearer only after commit.

A session lasts at most 12 hours. A password reset invalidates prior sessions
by advancing the credential revision. Current reads claim the account and
session before checking revision, revocation and fresh time. The returned
identity is a detached snapshot. Future protected writes must hold live session
authority in their own transaction. Rotation revokes the old bearer and
inserts one successor with the original expiry in a single transaction. Native
same-transaction custody and a deferred check bind both effects. An optional
private issuance path creates a distinct random synchronizer and stores only its
domain-separated digest in an immutable row in the session issuance transaction.
Protected logout matches that exact digest while holding the account and session
claims. Existing bearer-only operations remain private and compatible; a rotated
successor has no synchronizer binding and cannot use protected logout. Complete
secure delivery, blocklist, abuse limits and browser integration separately.
An optional test-only loopback login adapter requires preview authentication,
an exact HTTPS Origin and a separate admission key. The existing admitted login
performs quota debit before credential lookup and hashing. A successful response
sets a Secure, HttpOnly, host-only SameSite=Lax cookie bounded by the issued
session expiry and carries no account or bearer in its body. Failures carry no
cookie; an HTTP timeout can follow a committed session without returning it.
The original adapter supplies no synchronizer and its cookie cannot use protected
logout. A separately enabled test login issues a bound synchronizer once in a
no-store response and the bearer only in the secure cookie. Paired test logout
requires preview authorization, exact Origin, one canonical customer cookie and
one matching proof, and clears the cookie only after revocation commits. A lost
response can follow commit; duplicate logout denies rather than claiming a receipt.
Neither adapter grants session-read or public ingress authority.

### Private customer favourites

Hold current account and session claims before locking the per-account favourite
bucket. The database serializes membership changes on that bucket, enforces a
25-SKU cap and excludes fixture identities. The monotonic revision survives an
empty list, preventing a stale change from undoing a later one. A satisfied
request is a no-op rather than a durable command receipt. For adds and reads,
claim current publication sources using the existing projection. Recheck the
session after bucket and publication waits before returning private-state errors,
and recheck session and image rights at the final observed time. Only current SKU and
publication revision leave the private read; hidden entries remain removable.
The database writer is trusted for session and publication authority. No HTTP
route or customer erasure workflow is installed.

### Private restock opt-in

An explicit trusted call claims the live account and session, then serializes
its private subscription bucket before claiming current publication sources.
A current merchant SKU can have one active opt-in per account. The native bucket
advances for each actual insert or delete; a new row receives that revision as
its immutable generation and records its opt-in time. Unchanged requests do not
advance it, while a stale request cannot reverse a later change. Native guards
bound active rows to 25 and reject fixture identities. Final reads compare the
complete bounded state and recheck the session and current image rights.
Unavailable publication is hidden, but unsubscribe and clear still work. The
stock projection is not a transition claim or a reservation. Account email is
not copied. No notification event or transport is installed; future delivery
must separately establish generation-aware deduplication and cancellation/start
ordering. Current rows are not an audit or deletion/export policy.

### Private guest-order linking

Issue a 15-minute order-link proof only after checking the live guest basket
session, its CSRF proof and the exact immutable order in that basket. The
proof has an independent random bearer and verifier domain. Return it to
the trusted caller after the issuance transaction commits.

Redemption holds the current customer account and session claims, then
locks the exact proof and order. Compare the verified account mailbox to the
normalized buyer mailbox in the immutable order. A matching mailbox is one
condition of linkage alongside the order-specific bearer. Consume the proof
and insert one order-to-account link in the same transaction. The native
transaction identity and reciprocal checks bind both writes. The original
order remains immutable. Flush completion and recheck proof and session time
before commit.

Private order detail and history hold the same live account and session
claims through the read. They revalidate the reciprocal proof and link,
accepted order and verified buyer mailbox. The projection contains the order
identity and dates, item titles, quantities and captured amounts. It excludes
contact, address, payment and fulfilment data. Validate titles against the
exact approved publication and translation revisions used at checkout,
including the selected Dutch source approval for non-Dutch copy. Later
catalogue changes leave accepted titles intact. History seeks up to eight
entries in descending link time and order identity. The cursor is a live
position. Refresh from the beginning to include newer links. Browser
exposure and verified delivery remain separate.

### Credential admission

Acquire the credential purpose row before its target counter. Use a fresh
clock reading after that lock and commit the debit in a dedicated transaction
before starting authentication or proof work. Every worker uses the same
purpose budget and normalized-mailbox quota without looking up the account.
A later failure or cancellation of the credential operation leaves the debit
in place. A successful window rollover removes the expired counters for that
purpose and starts fresh counts in the same transaction. Never perform Argon2
or network work while holding quota locks.

Private login accepts an explicit trusted admission key and limits. After the
admission transaction commits, login uses the same observed clock for its
credential lookup and revision-bound session issuance.
