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
publication or validate the truth of a pricing reference.
