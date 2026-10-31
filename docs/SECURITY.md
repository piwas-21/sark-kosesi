# Security baseline

Use OWASP ASVS level 2 as the verification inventory at P1; map applicable controls to tests/review evidence and explain non-applicable controls. This is a verification target, not certification. Maintain a short threat model for customer/admin identity, money, shared inventory, provider callbacks, imports/uploads and infrastructure. Review it when a trust boundary changes.

- Rust enforces resource ownership and role permissions on every read/write/export. Test two customers, guest tokens, expired/revoked sessions and each admin role. MFA and recent reauthentication for privileged changes/refunds/exports; audit actor/action/object/reason without secrets.
- Validate all HTTP/provider/config input with bounds. Parameterized SQL only, constrained pagination/exports, request/body/time limits and safe error responses. Sessions use secure cookies, rotation and revocation; mutation CSRF/origin checks. CORS is an explicit allowlist. Passwords use a maintained Argon2id implementation; never invent crypto/auth protocols.
- Reset/verification tokens are random, hashed at rest, expiring and single-use; neutral enumeration-resistant responses. Rate limits cover login, reset, guest lookup, coupons, reviews, inbox and stock-alert subscriptions. Test bypass through IP/header spoofing and concurrent requests.
- Verify provider state server-to-server. Unique keys, bounded arithmetic, atomic stock and redemption transactions, outbox idempotency and reconciliations protect financial integrity. Never accept client totals or callback claims. No production DB/provider credentials in tests.
- Private uploads: authenticated ownership, byte/pixel/type quotas, decoded/re-encoded images, metadata stripping, malware policy for attachments, private storage and short-lived access URLs. Disallow arbitrary URL fetching; defend SSRF, decompression and image bombs. Room photos stay local unless consented.
- Escape output; no arbitrary rich HTML. Restrictive CSP with tested nonce/hash strategy, frame restrictions, MIME sniffing protection, referrer policy and HSTS after HTTPS validation. Protect cache/host boundaries and prevent secrets from entering public bundles.
- Logs redact tokens, cookies, payment data, addresses and message bodies by default. Correlation IDs connect bounded operational diagnostics; admin debugging cannot run shell/SQL or expose raw secrets. Audit retention differs from debug retention; privacy deletion preserves legally required financial records under approved policy.
- Data minimization, purpose/retention inventory, export/deletion workflow and consent separation. Privacy-safe operational telemetry; optional marketing tools remain disabled until consent requirements and value are approved.
- Pin dependencies, actions, images and toolchains; lockfiles and licence allowlists. No unaccepted high/critical vulnerability, leaked secret or unreviewed Sonar issue/hotspot. Lower severity exceptions need owner, expiry, affected version and mitigation; never waive payment/ownership invariants.
- Separate development/staging/production data, keys and databases. Rootless containers where supported, minimal images, health/resource limits, read-only filesystem where possible, restricted network/service exposure and least-privilege DB users. Backup encryption keys live outside backups and source.

P1 supplies automated authorization/header/abuse tests and a local-only scanner fixture. P6 runs an authenticated dynamic scan against isolated seeded staging and manual threat review. Do not scan production or trigger real orders/messages as a test. Release requires restored backup proof, rollback and incident procedures, dependency/SBOM review and exact-revision evidence.

Report vulnerabilities through the root [disclosure policy](../SECURITY.md). Do not include secrets or customer information in public issues/CI artifacts.

## P1 verification scope

ASVS level 2 is the inventory target. The implemented trust boundaries are the
private browser preview, thin same-origin Next proxy, Rust read API and isolated
PostgreSQL. Test data is invented and cannot be published by the schema; both
runtimes reject production configuration. Constant-time token checks guard the
preview; no real customer/admin session is implemented.

| Applicable area      | P1 evidence                                                                                   | Remaining acceptance                                    |
| -------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------- |
| Input/output         | Typed config, checked money, SQL parameters, decoded HTTP responses, Rust-generated contracts | P2 publication/import validation                        |
| Access control       | Missing/wrong preview credentials rejected; mutations and private commerce routes unavailable | P3/P4 customer ownership, roles, MFA, CSRF and sessions |
| Resource limits      | Bounded DB pool/readiness/request duration/concurrency; overload and locked-database controls | Measured production capacity/rate limits                |
| Browser security     | Nonce CSP, no framing, nosniff, no-referrer, no-store; raw anonymous/forged-host probes       | HTTPS/HSTS acceptance on chosen domain                  |
| Secrets/dependencies | Redacted errors, history/source Gitleaks, current RustSec/npm audit, licence/source policies  | Release SBOM/notices and secret provisioning            |
| Data isolation       | Every DB test creates its own disposable cluster; no public DB listener in native mode        | Staging/production backup and restore rehearsal         |

No customer accounts, write endpoints, upload processing, provider callbacks,
payment methods or outbound mail exist in P1. Their controls are deferred to their
owning phases, not marked passed or represented by mocks. Preview authentication
is not the future admin authorization system. The worker's default command probes
real DB readiness without effects. Its explicit local/test expiry command only
releases due hold reservations through the shared stock transition policy. Batch
limits, row claims, transactional rollback and durable redacted retry state bound
that capability; production configuration is rejected. It exposes no HTTP/admin
write route or arbitrary SQL execution. P3 owns outbox processing/recovery for
external effects.

P2 adds separate catalogue/warehouse capability checks on server-side staff
session records. Tests cover unknown/wrong-role/disabled/revoked/expired sessions,
missing/stale/future MFA, identity tampering and revocation or expiry during lock
waits. Authorization locks remain held through the stock transaction; caller
payloads cannot choose the audit actor, and warehouse changes require explicit
confirmation. Tokens are hashed at rest and have no
Debug/Serialize implementation. These tests prove the authorization consumer,
not a completed MFA ceremony: installation seeds no staff authority, and no staff
login/MFA/session issuer, cookie mutation route or admin write endpoint exists.
P4 owns those controls and P6/P7 own operational acceptance. Guarded draft price
writes require explicit confirmation, immutable evidence/reason/actor history,
optimistic revision checks and idempotency. Late expiry rolls back draft changes;
even receipt replay requires current staff authority. These records never grant
publication or validate the truth of a pricing reference. Specification writes use
the same authority/confirmation/history controls. Closed typed decoding, bounded
payloads and BOM validation protect staff reads from malformed direct imports.
A stored measurement, material or source reference does not verify its factual
truth or grant public use; no arbitrary reference URL is fetched. Media metadata
and permission history preserve immutable content classification, scoped terms,
revocation and audit identity; a new ID cannot promote known fixture/generated
digests. Rights reads reject malformed latest events and missing serialization
controls, and evaluate expiry after lock waits using the authorized clock sample.
The permission consumers alone do not verify image bytes. A separate guarded
private ingestion adapter now decodes/re-encodes bounded actual bytes, compares
claimed digest/codec/raw dimensions and excludes personal derivative metadata.
It performs CPU work outside transactions, with bounded concurrency and fresh
authority before effects/private return; immutable complete hash-bound sets are
stored atomically. There is no arbitrary URL fetching or caller-selected path.
No public storage/delivery or image publication is enabled. P4 still owns safe
HTTP upload/session/CSRF controls; P7 owns measured process memory/CPU isolation.

Copy review uses a separate, unseeded `catalogue.review` grant and rejects the
target draft's own author even if that account holds both grants. The recent-MFA
consumer and authority locks protect review effects and reads. Immutable decisions
bind exact copy/source/specification revisions, actor, evidence and qualification
references, with current-version checks for approvals and final clock checks for
every decision. Independent rejection can withdraw historical copy. References are not fetched
and cannot establish native qualification or semantic accuracy by themselves.
Historical replay cannot restore a later rejected decision. Corrupt copy or Dutch
source data fails private reads/new review; public publication and invalidation
must use validated assembled snapshots and remain unimplemented.

Catalogue authoring accepts bounded, closed commands under `catalogue.manage` with
confirmation, a supplier/source reference, a reason and final authority checks.
Actor identity comes only from the guarded session. Canonical command/effect
identity and immutable, complete creation events prevent silent replacement or
partial history. Known fixtures cannot be relabelled or combined with merchant
component pools. Candidate creation supplies no price, URL, stock or approval;
legacy preview copy rejects merchant rows. Source references are never fetched.
Supplier truth and publication eligibility require separate acceptance; default
installation still creates no staff authority and production stays disabled.
Permission, source/colour fidelity and SKU suitability remain human review inputs.

Independent factual decisions require the separate catalogue-review capability,
recent MFA and confirmation. Reviewers cannot approve their own identity, BOM,
specification or price work. Each decision binds exact immutable source versions,
current source checks for approval, expected review history, evidence,
qualification, reason and session-derived actor. Historical rejection withdraws an
older decision after source changes; replay returns a receipt without restoring
that decision. Private reads revalidate stored facts and enforce fresh authority.
Reference strings are untrusted records, never fetched and never proof of actual
supplier truth or qualified review. Public activation remains unimplemented.

Image suitability uses the separate review grant and rejects actors who authored
the identity, BOM, specification, media registration, ingestion or permissions.
Reviewer inspection reuses bounded derivative decoding and reauthorizes after CPU
work; originals and authoring operations retain their existing access boundary.
Decisions bind exact immutable sources and verified private bytes. Current-source
and fresh licence/session checks protect new approvals, with variant/asset locks
and SQL constraints spanning commit. Historical rejection and intent-bound replay
cannot silently restore withdrawn decisions. Attestation references do not prove
photographic truth or qualification and never enable public delivery by themselves.

Publication uses catalogue-management authority, confirmation, optimistic history
and a fully bound audit/effect intent. It consumes exact independently approved
source decisions, including the approved Dutch source of a translation, and never
trusts a caller's actor, current-eligibility flag or draft fallback. Fresh session
and licence checks after actual waits protect the atomic activation/path commit.
Private status locks before selecting the latest history; withdrawal and later
source/review/rights changes cannot be hidden by an earlier active row or receipt.
Raw schema imports retain source/version/decision guards and immutable route/event
binding. Full Rust source revalidation remains necessary alongside SQL checks.
Private audit/qualification/source records are not public product DTOs. Public
image delivery and projection/cache controls remain unimplemented; actual facts,
qualified review and production acceptance are separate evidence requirements.

Approved preview projection is an explicit whitelist, not serialization of private
review bundles. It revalidates exact current sources/decisions and product rights
under variant/asset claims, with a final clock after database work. Image delivery
repeats eligibility after bounded decoding outside the transaction and requires
the same publication and rendition identity. No original, asset ID, evidence,
actor, qualification, stock balance or storage path is emitted. Preview bearer
protection, loopback-only non-production configuration, noindex, no-store and
nosniff remain in force. Read/dependency/clock failures are redacted and never
become partial product facts. There is no shared product/image cache to invalidate.

Preview HTML/feed/image routes consume only decoded customer DTOs, never staff
bundles or private evidence. Unknown fields, wrong locale/SKU identities, malformed
or repeated cursors and dependency errors fail closed. XML escapes customer copy
and rejects text outside XML's character envelope rather than silently editing it.
The image proxy permits only exact revision-bound raster routes, forbids redirects,
and cancels upstream reads above the byte bound. It does not forward credentials,
private headers, arbitrary URLs or originals. Fixture merchant feeds and selling
schema are not advertised; noindex/no-store remain required. Browser authentication
remains the default, with the explicit public-staging read exception below.

Staging is an explicit non-selling preview mode. HTTPS canonical origins are
operator configuration, never derived from Host or forwarded headers. The API
remains loopback-only and token protected; PostgreSQL is on an isolated private
network with independent credentials. Browser access is private by default. An
explicit staging-only `STAGING_PUBLIC_STOREFRONT=true` permits only known storefront
GET/HEAD routes anonymously. Admin, unknown routes and mutations remain gated by
a separate `test-admin` credential; the internal API token cannot authorize that
browser gate. This shell gate grants no persisted staff account/capability and
does not replace future staff sessions/MFA. Exact revision headers bind responses to the
reviewed image. Live commerce configuration remains rejected. A deployment smoke
checks actual TLS, the chosen storefront access mode, protected admin access,
private-cache/indexing headers, locale redirect,
RTL and the disabled-production catalogue; it performs no financial mutation.

Market policies use separate unseeded author/reviewer capabilities and the existing
recent-MFA transaction guard. Policy input and stored JSON are closed and bounded;
no customer request can supply readiness, tax or shipping amounts. Independent
exact-revision approval and latest-draft selection prevent old-policy fallback.
An anchor claim spans reviewed quote catalogue/stock reads, serializing policy
withdrawal; final clocks reject expiry after waits. Immutable SQL history also
rejects raw-import gaps, self-review and stale approvals. These private consumers
add no HTTP write/session issuance or payment permission, and evidence references
still require actual qualified tax/carrier/territory review before destination use.

Indicative promotion terms remain trusted, unseeded server input. The closed
quote request has no coupon, amount, eligibility or budget-approval field. Promoted
quotes reuse reviewed market/publication/stock claims and final source clocks;
promotion expiry cannot extend country or photography eligibility. Stored quote
arithmetic is revalidated before discounting, and every amount remains bounded EUR
minor units. Refund arithmetic preserves immutable captured tax but grants no
finance capability, ownership, provider attempt or restocking effect. Atomic
redemption limits and authenticated checkout remain prerequisites.

Guest basket credentials use a maintained stateful transport and a private SQLx
store. The opt-in internal routes retain preview authentication and a validated
HTTPS Origin; installation grants no guest/staff/order authority. Mutations reject
missing, duplicate or foreign Origin and session-scoped CSRF headers, and reject
nonobject/unknown input and oversized bodies. Secure, HttpOnly host cookies carry
only an opaque identifier; the database retains a domain-separated SHA-256
verifier. Cookie removal is normalized to preserve the host prefix attributes.

Fixed lifetime and immutable identity bindings cannot be extended or upserted by
framework saves. Every operation freshly claims the credential; revocation claims
it exclusively through commit. Basket receipts bind normalized full intent and
optimistic revision, so retries do not reverse newer content. Atomic minute
ceilings bound issuance and mutation; forwarded headers cannot reset the limits
or choose Origin. This is not per-IP bot protection: public exposure still needs
an approved trusted-edge policy, retention/deletion operations and complete
checkout authority. Logs expose static failure codes and correlation IDs, with
positive/negative capture controls for cookies, CSRF and body values. No public
customer bridge or order/payment effect is enabled by these routes.

Promotion policy writes require their own unseeded manage/review capabilities,
recent MFA and confirmation. Market/catalogue authority does not imply promotion
authority. A different actor reviews exact immutable source terms; effect replay
cannot reverse a newer withdrawal or draft. Bounded closed stored objects and
canonical identity/limits are revalidated. A shared borrowed observation clock
covers staff checks, waits and final writes; revocation retains its exclusive
ordering. No customer can submit discount amounts, policy, limits or role claims.
Configuration has no budget-spending or payment authority. Verified customer
limits cannot be satisfied by treating a guest cookie as a person. Current
promotion readers remain trusted private primitives without HTTP exposure.

Whole-basket manifests are trusted stock primitives, with no customer mutation
route. Opaque group/child identifiers and server order ownership cannot select
customer roles or publish products. Immutable insert-time child/group bindings
and deferred completeness checks prevent legacy adoption, partial groups and
reuse of a hold across groups. Exact normalized retries bind every member and
window; they never reactivate expired/released/consumed history. Global component
claims span aggregate demand and all effects, and fresh final clocks deny expiry
or rewind after waits. Later checkout must supply fresh guest authority, approved
publications/policies, totals, complete order/outbox atomicity and final clocks.
No payment or fulfilment permission follows from a trusted reservation receipt.

Private guest quote routes reuse the existing credential, Origin/CSRF and preview
boundary. Owner claims precede stored quote lookup; a quote ID never authenticates
a reader. Requests cannot supply owner, lines, prices, policy, tax, stock or roles.
Closed bounded JSON and static errors prevent internal source/destination exposure.
Snapshots and command inputs omit Debug; customer projections omit owner, policy
and destination. Stored JSON is size-bounded and allocation/shape/header validated.
Immutable rows cannot be edited/deleted or rebound to another basket/review/BOM.
Final source/identity checks follow inserts; revocation waits for active claims,
and rollback leaves no quote or successful-issuance charge. Historical receipts
cannot refresh expiry or bypass current order readiness. Public bridge abuse
controls, retention operations and complete order/payment authority remain required.

The time-bounded development-only minimatch advisory disposition is recorded in
[dependency review](DEPENDENCIES.md#temporary-development-tooling-exception).
It expires at 2026-11-03T00:00:00Z, preserves visible moderate audit findings and existing
CI timeouts, and grants no ownership/payment, high/critical or production waiver.

Private order creation accepts intent only through a trusted infrastructure entry.
Fresh guest ownership/CSRF and current reviewed sources protect an atomic order,
complete component reservation and minimal PII-free outbox event. Buyer/address/
seller snapshots intentionally lack Debug; the boundary maps database errors to
static classes and never returns failed-row detail. Stored bounded closed objects
and source/header bindings are revalidated on reads and retries. Address/email
syntax is neither ownership nor delivery proof. Seller/legal snapshots require
trusted configuration; installation supplies none. Public routes, abuse controls,
retention/erasure operations, coupon accounting and payment authority remain gates.

The private Mollie test client accepts no live key or caller-controlled provider
origin. Raw authorization is sensitive and excluded from Debug/serialization and
database snapshots. Profile preflight is required before create/fetch capability.
No proxy, redirect, automatic replay or decompression is enabled; HTTP deadlines,
65536-byte responses, closed critical objects, exact decimal EUR and fetched
profile/metadata/sequence checks protect the provider boundary. Client errors are
static and discard URLs, keys, request/response detail and provider PII. Optional
checkout links are HTTPS/host validated independently of financial evidence.
Create requires the consumed durable permit; errors retain reconciliation.
Rotated verified same-profile keys may read historical payments but cannot create
under the old configuration. The library installs no callback, runtime key,
worker, payment transition or production activation. Loopback test configuration
is compiled only for unit tests and rejects every non-loopback origin.

Private list discovery retains the verified-profile capability and read-only
rotation policy. Each call is bounded to ten results and existing HTTP budgets;
continuation data cannot choose a request origin/path or add profile/mode inputs.
Explicit object-shape checks reject nested positional-array substitutions;
original JSON entries preserve duplicate critical-field rejection. Either matching
order/attempt metadata identifier requires complete financial matching; unrelated
metadata and provider PII never become stored observations. Multiple identities
remain explicit candidates for separate GETs. Empty/unfinished discovery cannot
end uncertainty or grant a replacement create. Durable cursor/observation custody,
job budgets and authenticated callback processing remain unimplemented.
