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

The RustSec check audits the full lockfile. The PostgreSQL-only SQLx export
crate removes the unused SQLite dependency affected by
[RUSTSEC-2026-0459](https://rustsec.org/advisories/RUSTSEC-2026-0459.html).
Failed audits preserve their JSON report separately from passing evidence.

## Private favourites boundary

The private SQLx operation accepts only a current customer session, never a
caller-selected account. It retains only stable merchant SKU identities, uses
a bounded account-specific bucket and returns currently publishable identities
only. Database guards serialize and cap native membership changes; they do not
authenticate native SQL callers or prove publication. An unavailable saved
entry is hidden and remains removable. Session validity is rechecked after
bucket and source waits before reporting revision conflict or capacity. A bucket
remains after clearing to preserve revision ordering. Export, deletion, retention and browser CSRF and
abuse controls require separate implementation and review before exposure.

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
finance capability, ownership, provider attempt or restocking effect.

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

Only trusted private code selects a promotion for an owned indicative quote.
Keep the same guest ownership, CSRF and issuance limits; a client-supplied
promotion, customer identity or amount confers no authority. The latest exact
independently approved promotion must still be eligible after source waits.
Historical replay validates the exact original review, amounts, intent and
expiry. The legacy projection rejects promoted quotes. Private promoted order
acceptance requires the exact current approval and a same-transaction debit.
The promotion anchor serializes capacity claims and approval changes. The
immutable ledger keeps usage across revisions and prevents order or quote reuse.
The database independently verifies the original publication price, market VAT
and shipping, and the approved discount formula and allocation before recording
a debit.
Guest checkout rejects a policy that requires a verified customer allowance.
Private account-bound quote issue and replay require guest ownership, the same
verified account, and a current credential-revision-bound session through the
final time check. The account association stays in immutable private storage.
Verified acceptance requires the exact account-bound quote, held current
authority and a reciprocal account debit in the order transaction. Guest
acceptance, guest payment preparation and history, and guest order-link proof
issuance reject account-bound custody. Private verified payment preparation and
history require both the retained order account and current guest ownership.
Native insertion binds the attempt to the quote account. The account association
stays in the private relational record.
Private cancellation uses the same guest and account authority as the order.
Cancellation and admission share a basket claim; cancellation and dispatch share
an attempt claim and reject a conflicting durable effect after waiting. The event
carries only the order ID and schema version.
Public coupon access still requires authenticated checkout and trusted-edge
abuse controls.

The development-only minimatch exception expired on 2026-11-03. The
[dependency review](DEPENDENCIES.md#development-minimatch-advisory) documents
the verified lint backport and removal of the separate OpenAPI dependency.
The npm audit remains visible. CI timeouts and production security gates still
apply.

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
end uncertainty or grant a replacement create. Durable cursor/job custody,
job budgets and authenticated callback processing remain unimplemented.

Private observation recording consumes the authenticated-fetch capability and
revalidates original order/attempt/dispatch money, profile and metadata before
persisting a minimal closed projection. The actual fetched-key fingerprint is
audited; no raw key, provider details, address or checkout capability is stored.
Observations have no Debug, financial-state, ownership or fulfilment authority.
They retain late/reordered/different-ID evidence without selecting a winner.
Trusted internal reads require bounded closed headers/snapshots and the exact
processing event; they are not customer routes. New writes serialize per attempt,
bind full-intent retries, check final monotonic time and commit the immutable fact
with its PII-free event. Database errors discard query/detail data. No network
operation occurs in this transaction. Identity resolution, supervised processing,
customer ownership wrappers and all actual provider/release gates remain required.

Private search custody accepts only opaque authenticated list results bound to the
entire original frozen attempt and requested cursor. Actual key fingerprints and
minimal identity hints are retained; provider response bodies, raw keys and buyer
information are excluded. Continuation requires exact immutable predecessor
custody, rejects forks/cycles and caps each scan at 100 pages. An end/empty/capped
scan cannot prove absence, authorize a replacement create or change payment/stock
state. Receipt retries and processing events are atomic; runtime workers,
callbacks, supervised escalation and identity resolution remain required.

Private transient payment-recovery decisions require the separate unseeded
`payment.reconcile` finance capability and fresh persisted session/MFA authority,
held against revocation through commit. Confirmation, exact stopped generation,
original profile and a bounded incident reason bind the action. Immutable receipts
and the exact job change commit together; replay requires current authority and
cannot restart subsequent progress. Three lifetime resumes retain all hard work
budgets. Paid/ambiguous/capped/invalid-custody stops remain blocked. Errors discard
raw storage detail. This adds no public endpoint or charge/refund/stock authority.

Private callback-ID lookup exposes only a verified GET-only test client. Returned
metadata is an original-attempt lookup hint, never ownership or payment authority.
Exact authenticated profile/resource/ID, closed mode/currency and bounded metadata
checks precede the opaque result. Full frozen order/attempt/dispatch matching is
required before the existing atomic observation/outbox append; optional checkout
capabilities and provider details are discarded. Duplicate critical JSON fields,
wrong scope and forged original financial terms fail closed. No callback HTTP
route or inbox is exposed. Durable private lookup operations now bind opaque
committed server identities and durable single-use starts to GETs, recover
receipts before I/O and retain authenticated unmatched results in a separate
callback aggregate. Minimal closed snapshots, exact operation/header/event bindings
and immutable native guards
are required before private stored-capability reconstruction. Complete lookup
snapshot SHA-256 binding in the immutable event also rejects otherwise-valid status,
amount or fingerprint changes in imported data. Complete original matching still
precedes order-associated observations; namespaced receipt replay
cannot turn an unknown attempt into an order or replace a prior fetched fact.
Private inbox admission now commits minimal immutable intake/source/job custody
under atomic global/profile rates and active-capacity limits. Native guarded
independent active custody prevents completed history from making scans unbounded
or a damaged settlement flag from hiding work;
older unresolved generations and review dispositions cannot release their slot.
Claims bind exact generations to one-use GET operations before I/O. Immutable
consumed-claim receipts prevent historical lease reuse after checkpoint damage.
Only current
leases can advance checkpoints, while independently authenticated late facts may
still enter durable receipt processing. Stored dispositions must revalidate the
lookup digest, original observation and identity-resolution custody. No saved job
or outcome string creates authenticated payment evidence. Fresh processing cannot
precede the saved receipt's time. A custody-invalid exception requires a real raw
receipt and complete independent intake/generation source, contains no observation
or resolution authority, and cannot premark an in-flight GET. Damaged receipt or
deterministic identity-history failures stop that operation visibly while other
receipts can progress. Damaged proof behind an existing immutable disposition
creates a separate source-only quarantine, without changing the original outcome
or payment facts. Quarantine retains capacity and review status even after exact
proof repair; it grants no financial authority. Bounded retries stop
visibly and unchanged later worker passes retain failure status. There is no
public receiver, runtime key or scheduler; bounded parsing, trusted-edge abuse
controls and staging acceptance remain required before operational notifications.

The optional private classic-form adapter retains preview Bearer authentication,
loopback binding and rejected production startup. Its profile is validated server
configuration; neither form nor URL parameters provide profile, mode or payment
authority. A closed 1024-byte form and bounded Content-Type policy reject duplicate
decoded IDs, extra fields, malformed percent/UTF-8 input and content encoding.
Only durable intake commit permits the indistinguishable empty success response.
Errors, responses and request logging contain no body, provider ID, authorization
or cookie values. Public staging POST access and trusted-edge controls remain
unconfigured; this adapter does not authenticate a provider notification.

The optional staging edge validates the actual TCP connection peer against exact
configured private proxy addresses before consuming a body. It never trusts XFF,
Host or a budget-group header for identity. This is a network ACL, rather than
cryptographic proxy or provider authentication; unique mapping and no-bypass
network evidence remain deployment gates. Whole subnets and shared SNAT gateways
are not acceptable proxy identities.

The forwarder uses a fixed validated loopback target, fresh sensitive Bearer and
canonical form headers, and no environment proxy, redirects or POST retries.
Incoming credentials/cookies/forwarding fields and upstream diagnostics never
cross that boundary. Only a completed empty private-API 200 permits success.
Global concurrency, streaming byte and deadline bounds limit admitted work. The
optional Traefik route and process are off by default, production remains rejected,
and no secret is stored in routing labels. Its nonsensitive overwritten inflight
group is only a resource bucket. Source tests cannot establish the actual proxy's
read timeout, network identity or provider delivery.

## Private stock scarcity retry

The private scarcity retry requires an unseeded `stock.recover` grant and recent
MFA. Explicit confirmation binds the exact case revision, next generation and
effect key. The transaction retains the authority and payment attempt locks
through the decision and its final time checks. A still-scarce decision cannot
restart automatically. Native custody binds the actor account, exact historical
review, fresh received evidence and reciprocal decision, claim and effects.
Database write-role restrictions still apply. The outbox projection carries
only the decision and order identities and a digest, with no operator reason.

## Private refund reservation custody

Use an unseeded `refund.prepare` grant, recent MFA and explicit confirmation of
the order, expected received recognition and each requested amount. The server
derives the audit actor from the locked session and rechecks time before commit.
Reserve only against the authenticated, exact accepted EUR total and the
serialized reservation history. Exact receipt replay requires current authority
and the original intent, including the actor, with historical payment custody.
The database role stays trusted. The event carries only the order, reservation
and digest. Keep raw reasons and credentials out of diagnostics. Execution,
credits and inventory effects require independent authority.

Preparing a refund execution proposal requires the separate unseeded
`refund.execution.prepare` grant, recent MFA and explicit confirmation. The
command derives the profile and payment identity from authenticated historical
evidence; caller input supplies only the expected internal source and watermark.
Claim the attempt before checking current recognition. Hold the staff session
and grant through commit and recheck staff time after waits. An exact historical
retry remains actor-bound and requires live authority. The outbox carries
only the order, proposal and digest; keep reasons, payment/profile IDs and raw
SQL errors out of diagnostics. No credentials or provider dispatch are installed.

## Private provider reversal observation custody

Only an authenticated exact-item result from the test-profile client enters the
observation writer. Network I/O finishes before the transaction claims the
dispatched attempt. The writer verifies the frozen source and keeps a bounded,
immutable history with exact replay and a reciprocal event bound to the full
receipt digest. Keep provider metadata, reasons, raw credentials and raw SQL
errors out of observations and diagnostics. The PostgreSQL writer role remains trusted for provider provenance.
The journal preserves positive provider facts and leaves current financial,
stock and fulfilment decisions to separately authorized operations.

The paid-parent aggregate GET uses a single committed permit tied to the exact
received source and authenticated test credential. The credential fingerprint is
private and binds the fetched fact to the operation. The durable event carries
the order, operation and digest. Keep raw credentials and provider identifiers
out of event payloads and diagnostics.

The private page journal accepts only a verified payment-scoped list result.
The attempt lock serializes page admission. The native journal enforces bounded
history, one successor per predecessor and exact continuation scope. Receipt
recovery checks the reciprocal event digest and bounded ancestry before returning
historical evidence or restoring an open tip's opaque cursor. A continued fetch
rechecks the paid parent and actual test-key fingerprint. Candidate identities
require exact authenticated item reads before they enter the observation journal.

The private supervisor revalidates the paid source, frozen dispatch and exact
receipt during enrollment and claim. The durable start fixes the generation,
credential fingerprint and predecessor before either GET. A stale worker can
retain late provider evidence. Only the current lease or exact receipt recovery
can advance a checkpoint. Expiry without a committed receipt leaves an explicit
review state. A new key uses a separate scan with a fresh root.

Item supervision revalidates page ancestry, the reciprocal page digest and the
one-based candidate ordinal under the attempt claim. The exact paid parent and
child are fetched under the same verified test key outside the claim. Completion
requires the operation's own reciprocal journal receipt with matching child,
collection and key. A receipt from another operation cannot advance the job.
Expired operations preserve their evidence for review without starting another
fetch for the same operation.

## Private email verification challenge custody

The private challenge primitive accepts only ASCII dot-atom local parts and ASCII
DNS domains. It preserves the local part's case, dots and plus tags exactly; only
ASCII domain case is folded. It rejects Unicode, quoted local parts, address
literals, single-label domains and malformed/oversized input. This deliberately
narrow identity policy does not infer provider-specific aliases or prove delivery.

The server generates 256 random bits. The direct issuer returns the bearer to
the trusted caller. PostgreSQL retains its SHA-256 verifier, with a distinct
versioned purpose domain, alongside the normalized identity and fixed
15-minute lifetime.
Consumption serializes on the verifier and requires the same purpose and identity;
server time is checked after lock waits and again after the update. An invalid,
mismatched, expired or consumed challenge has the same denial. The delivery
issuer stores a bounded authenticated ciphertext and returns a receipt without
the bearer. It uses a separate versioned key generated by a cryptographic
random source and a synchronously committed nonce reservation bound to the
exact verifier. Each key has at most one million reservations. A failed issuance
leaves its nonce burned. Store keys separately from database
backups and keep plaintext bearers and raw keys out of logs and storage.
A separate private start accepts only an exact live envelope with its registered
key fingerprint and previously committed nonce. It creates one durable record
and emits no bearer, ciphertext or mailbox. A trusted in-process consumer can
receive borrowed bearer bytes and the normalized mailbox only after its own new
start commits, the source is rechecked under lock and the ciphertext and
recovered verifier authenticate. No transaction spans the callback. Expiry or
consumption can still occur after the last local check. The caller must keep the
borrowed values out of logs and storage. A committed start with no separate
outcome stays unknown; replay cannot reopen it.
An optional trusted in-process callback returns only accepted, explicit rejection
or uncertain. An immutable durable row binds that classification to its exact
earlier start without storing a mailbox, bearer, credentials or provider body.
Recording happens after the callback, outside its I/O, and does not require the
challenge to remain live. The database records the trusted caller's classification.
Provider authentication, mailbox receipt and redemption each require their own
evidence. The in-process caller and write role remain trusted. A crash or failed recording
leaves the start unknown and never permits reopening. Acceptance proves neither
mailbox receipt nor redemption. No provider or retry is installed.
Anti-enumeration, token transport, mailbox delivery and recovery remain separate
requirements. Private verified account establishment composes the exact
challenge consumption and a unique immutable normalized identity in one transaction.
The database assigns the consumption transaction ID and requires the account insert
to match; legacy or separately committed consumption cannot be adopted. This attests
transaction custody, not possession: Rust verifies the bearer, and the database write
role must remain trusted and inaccessible to untrusted clients.
A duplicate identity or invalid proof denies and rolls back; suppressed writes,
clock failures and expiry after a lock wait also cannot create detached success.
The separately committing consume primitive cannot bridge a later account write.
An account identifier grants no login, session, guest-order or production authority.

The optional test-only redemption POST requires preview Bearer authentication, a
single exact configured HTTPS Origin, bounded closed JSON and a canonical 32-byte
proof in lowercase hex. Its origin and nonzero admission key must be supplied
together in test mode; the normal API bind remains loopback-only. It returns no
identity or credential. Invalid, expired, mismatched, replayed, duplicate and
throttled proofs share a denial; storage failure has a separate redacted response.
The shared request timeout can occur after a commit and does not prove rollback.
This consumer does not provide delivery or establish public abuse or timing safety.

## Private password setup and recovery custody

Generate each proof from 256 random bits and use a verifier domain separate
from registration, guest and staff credentials. The proof is specific to one
verified account and its current credential revision. Rust verifies possession.
PostgreSQL rejects detached consumption, wrong account or revision and reused
transaction proof. Lock and expiry checks protect simultaneous replacements.

Normalize passwords with NFC before hashing and retain spaces. The private
primitive requires at least 15 normalized Unicode code points and at most 256
UTF-8 bytes before and after normalization. Use Argon2id v19, 19 MiB, two passes,
one lane, a random 16-byte salt and a 32-byte digest. Bound concurrent hashing
and keep passwords, bearer values, hashes and SQL details out of diagnostics.
Only the current hash remains in the live credential table. Backup retention
and deletion still require the release privacy policy.

Every private replacement requires a caller-supplied immutable offline blocklist
snapshot. It contains 1–1,000,000 strictly sorted, unique SHA-256 digests of
NFC-normalized UTF-8 passwords, bounded version/provenance/qualification
references and a trusted validity interval of at most 90 days. The caller passes
an independently trusted SHA-256 integrity digest. The canonical digest covers
`piwas:password-blocklist:snapshot:v1` and a NUL, then each metadata string's
unsigned 64-bit big-endian byte length and bytes, followed by signed 64-bit
big-endian validity bounds, unsigned 64-bit big-endian entry count and the
concatenated 32-byte entries. Snapshot validation establishes integrity and
shape, not authenticity, actual corpus quality or reviewer qualification.

After the admission debit and cheap revision-bound proof precheck, release the
database transaction and reject exact blocklist matches before Argon2. Normalize
with the same NFC function used for hashing, retaining leading and trailing
spaces. Missing, malformed, not-yet-valid or expired snapshots fail closed.
Recheck expiry with the final observed clock before committing the credential;
rejection leaves proof and credential intact but never refunds admission.
Coordinate trusted snapshot provisioning, corpus review, withdrawal and freshness
across instances; an already supplied immutable snapshot has no live revocation
channel. No corpus or grant of production readiness is included. Public recovery
also requires secure delivery, uniform responses and timing, safe transport and
distributed abuse controls.

## Private reversal-item review inspection

One exact review stop can be inspected with a dedicated unseeded `reversal.review`
grant and fresh MFA. Staff and attempt claims are held while source, checkpoint
and any late receipt are validated; time is rechecked after waits. The detached
summary contains only a closed reason, generation and late-receipt presence.
It authorizes neither provider access, retry nor any financial action. Public
operator access, audit operations and provider reconciliation remain separate.

## Private customer sessions

Issue 256-bit random bearers after verifying the exact current credential.
Use a SHA-256 verifier domain distinct from password proof and staff or guest
tokens. Never log or persist bearer bytes. Hash verification runs outside the
database transaction and only accepts the reviewed Argon2id profile. A dummy
hash covers absent credentials with otherwise valid inputs. Public
anti-enumeration behavior and ingress rate controls require separate checks.

Claim the account before the session, and reject stale credential revisions,
revoked sessions and expiry after waits. Rotation creates one reciprocal
successor without extending the original expiry. Flush completion before the
last clock check. Logout revokes the current bearer. Browser cookies, Origin
and CSRF integration remain separate.

An optional default-off test-mode login POST requires preview authentication,
one exact configured HTTPS Origin, closed bounded JSON and a separately configured
admission key. Existing login admission commits before credential lookup or Argon2.
Unknown mailboxes, wrong passwords and exhausted quota share a denial. Success
returns only a Secure, HttpOnly, host-only SameSite=Lax session cookie bounded by
the original expiry. Response bodies and logs contain no bearer, password or
account ID; the session bearer is present in the sensitive Set-Cookie header.
The request deadline may follow a committed session without delivering its cookie.
No public timing, trusted-edge abuse or session-read policy follows from this test.

Optional private issuance can bind a random independent synchronizer digest to
the exact new session in its creation transaction. Its bearer and proof return
only to the trusted caller; do not log or persist their raw bytes. Protected
logout compares the digest in constant time after claiming the live account and
session, in the same transaction as revocation. Existing bearer-only operations
remain private; a legacy or rotated unbound session fails protected logout.
No HTTP response, Origin policy, token recovery or public CSRF protection follows
from this custody primitive. The test login does not use this issuance path or
expose a synchronizer.

## Private guest-order linking

Use distinct proof and customer-session bearers. The issuer checks a live
guest credential, its CSRF token and the accepted order in the owned basket.
Redemption holds the current revision-bound customer session through commit.
It requires the exact proof, order and verified buyer mailbox. Preserve the
local part and fold only the ASCII domain through the existing normalizer.
Keep bearer bytes and buyer information out of link records and diagnostics.
PostgreSQL requires reciprocal consumption and a unique immutable link.
Wrong scope, expiry or a competing link leaves the proof unspent. The
private adapter provides no browser route or proof-delivery transport.
Read linked orders only under live account and session claims. Filter the
link by the session account, validate its exact consumed proof and accepted
order, and check session time again before returning. Keep buyer, delivery
address, payment and fulfilment details out of the history projection.

### Private credential admission

Reserve capacity before expensive credential work using a separate global
and target budget per purpose. Normalize with the same account identity rule
and use a purpose- and window-specific HMAC-SHA256 digest of that mailbox. Malformed
input shares a dedicated target bucket. Apply the policy before account
lookup so unknown and known mailboxes consume the same budgets. Counters
contain neither plaintext mailboxes nor the key.

The login operation requires admission before any credential lookup or Argon2
work. Exhausted quotas, unknown identities and incorrect passwords use the
same denial. An admission storage or clock failure prevents login. This
classification does not establish equivalent response timing.

Email proof issuance and consumption, account establishment, password proof
issuance and password replacement each require a committed debit before proof
work. Standalone email proof consumption and account establishment use the
same budget. A failed request retains its debit while the proof and any
account or credential writes remain governed by their own transaction.

Run all instances with the same trusted key version and limits. The active
60-second window pins both. Switch configuration at a window boundary and
coordinate key version changes across instances. Each purpose rejects a
version below the one it last admitted and rejects a changed key using the
same version. Under the purpose lock, rollover must leave no old targets and
each debit must read back the exact target counter and scope policy, key,
window, count and time before it can succeed. Verify the same durable state
after a high-water update; a mismatch rolls the transaction back. Recheck
monotonic time after lock waits and preserve the
post-write sample as the durable high-water mark. Check time again after
commit. A late result denies permission while retaining the debit.
One active window per purpose bounds storage. An inactive purpose's expired
digests persist until its next successful rollover, so time-based purge
remains an operational requirement before public exposure.

Complete trusted source limits at ingress, neutral responses, enumeration
and timing tests, and measured overload budgets before wiring public routes.
The per-target cap can temporarily restrict an account when someone else
submits its address. Treat that as an abuse case in ingress acceptance.
