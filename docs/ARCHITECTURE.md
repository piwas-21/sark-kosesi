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
migrations/              # forward SQL migrations
packages/contracts/      # generated TS client/schema
packages/design/         # tokens and accessible UI primitives
 tests/                   # integration, contracts, browser, security, load
```

Keep modules small and organized by commerce capability. Avoid a generic repository/CQRS framework. Domain code is independent of HTTP/SQL and is deterministic with explicit clock/identifier inputs. Use typed identifiers, checked integer money arithmetic and explicit state transitions; no floating-point money, unchecked casts or panic/unwrap/expect in request paths. Forbid unsafe code in owned application crates. Error responses are typed and redact internals. Connection, request, payload, queue and retry limits are explicit.

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
excluded. Animation is rejected. New encoders preserve alpha where applicable and
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

## Payments and durable work

Use a dedicated approved Piwas retail website profile and keys in the existing Mollie organization only if Mollie confirms eligibility. Do not copy Sofra's subscription keys or payment logic. Start with implemented payment methods only; hosted provider checkout reduces card-data scope. Customer redirects never establish payment success.

Create a stable payment attempt and idempotency key before calling Mollie. Do not hold a DB transaction during provider I/O. Unknown create/capture/refund outcomes retain their original attempt/key and enter reconciliation; never mint a replacement charge. Fetch webhook identifiers with the correct server credentials and verify order, amount, currency, environment and profile. Deduplicate and tolerate delayed, repeated and reordered notifications.

Order, payment, fulfilment and return states are separate. Late paid payments after reservation expiry attempt atomic reallocation; failure creates a visible finance/support exception and recovery/refund process. Payment success cannot be discarded because stock expired. Partial refunds cannot exceed captured less refunded amounts; preserve tax/discount allocation and credit-document links. A refund does not automatically restock goods. Chargebacks are finance events.

Write domain changes and outbox events in one transaction. Workers claim leases using PostgreSQL, retry with backoff and stable effect identifiers, and recover abandoned leases. Emails, stock alerts and fulfilment requests are idempotent. Record provider acceptance separately from customer delivery. Failed jobs are visible and replayable only through bounded, audited admin actions.

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
