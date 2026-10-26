# Security, privacy and reliability baseline

Target: applicable **OWASP ASVS 5.0.0 Level 2** controls for the production commerce system. This is an engineering verification target, not current compliance. P1 must import the versioned requirement list into an applicability matrix, with owner, implementation, automated/manual evidence and justified not-applicable entries. Do not invent ASVS identifiers. [OWASP ASVS](https://asvs.owasp.org/).

## Trust boundaries and abuse cases

| Boundary | Threat | Required control and evidence |
| --- | --- | --- |
| Browser → quote/cart | Changed prices, coupon stacking, stock spoofing | Server catalogue/tax/shipping/discount calculation; immutable quote expiry; tampering and race tests |
| Customer → order/account | IDOR, guest token guessing, enumeration | Ownership on every read/write/export; high-entropy scoped expiring guest access; generic recovery responses; two-account negative fixtures |
| Admin → privileged action | Stolen session, excess permissions, replayed refund | MFA, role matrix, revocation, step-up for finance/access, bounded idempotent mutations and audit trail |
| Mollie → webhook/return | Forged IDs, wrong profile/mode/amount, replay and late payment | Fetch from configured profile; match attempt/order/currency/amount/mode; independent state transition; no payment truth from browser return |
| Concurrent checkout → DB | Oversell, deadlocks, double allocation | Atomic component reservations, stable request keys, retryable conflict policy; real PostgreSQL contention tests |
| Worker → provider | Duplicate emails/refunds, lost acknowledgement | Transactional outbox, unique event keys, provider idempotency, reconciliation, bounded retries and dead letters |
| Rich text/media → reader/server | Stored XSS, executable upload, SSRF, decompression bombs | Allowlisted sanitisation; MIME/signature/size/dimension limits; re-encoding/metadata removal; private quarantine; no arbitrary URL fetch |
| Logs/support/photos → operators | PII leak, public links, excessive retention | Structured field allowlist, query/body redaction, scoped access, signed short-lived URLs, retention/deletion jobs and synthetic sentinel tests |
| CI/dependency → runtime | Stolen tokens, malicious scripts, untrusted artifact | SHA-pinned actions, read-only job tokens, no production PR secrets, locked installs, explicit build-script allowlist, SBOM/image verification |
| Environment → commerce data | Cross-environment access, backup leak, failed restore | Independent DB/role/buckets/keys, non-root containers, encryption, restore drill and measured recovery |

## Production requirements

- Secure, HttpOnly cookies with appropriate SameSite policy; session rotation after authentication/privilege changes, logout/revocation and bounded lifetimes. Choose credential/passkey/OAuth design in P1; do not copy an unreviewed beta auth implementation.
- Validate CSRF/origin for cookie-authenticated mutations. CORS allowlists are explicit. Rate-limit credential recovery, registration, stock alerts, coupons, support and expensive uploads. Provider callbacks get a retry-safe policy, not a user CAPTCHA.
- CSP with tested nonce/hash strategy, frame-ancestors, nosniff, referrer policy and minimal permissions policy; HSTS after HTTPS/domain validation. Preview inline scripts are not the production CSP template.
- Parameterised persistence, explicit validation and safe output encoding. No arbitrary SQL/shell/debug console in admin. User-facing errors contain safe context and correlation IDs, never stacks, keys or raw provider payloads.
- Every admin capability has allowed/denied roles and resource-level tests. Owner, operations, support, content and analyst permissions are distinct; customer claims never grant admin rights.
- Hosted payment flow only; no card data storage or proxying. Verify applicable provider/PCI obligations separately. Never combine payment, fulfilment and returns into one overloaded status.
- HTTPS-only provider URLs from validated configuration, bounded timeouts, retry/backoff and circuit protection. No DB locks held during network I/O. Webhook verification failure must not allocate stock or notify a paid order.
- Favourites/alerts/marketing consent are separate purposes. Email verification and an opaque unsubscribe/claim link prevent other people being subscribed silently. Purchase and business-record retention differ from account deletion.
- Room photos remain on-device unless explicit cloud-save scope is added. Cloud storage requires per-user authorisation, retention/deletion including derivatives, safe processing and separate training consent. No photos in logs, test artifacts or error replay.
- Diagnostic retention target 30 days, audit/accounting retention subject to the approved policy; these are different classes. Record processors, locations, purpose, legal basis and export/delete behavior. Obtain specialist review for legal obligations, not a guessed retention number.

## Security testing and response

CI secret and high/critical dependency findings block merges. CodeQL alerts must be triaged against the actual PR analysis, not just a green upload step. P1 adds container/IaC scans, licence checks and SBOM generation; P5 adds authenticated DAST in an isolated staging environment plus manual business-logic review. Do not scan third-party systems or real customers.

Security tests include cross-account order/return access, guest token scope, expired session, CSRF, coupon abuse, webhook mismatch/replay, unsafe image/text inputs, public cache leakage and PII sentinels. Pair denied cases with an allowed case so a universally broken endpoint cannot look secure.

No secret exceptions. A non-exploitable dependency exception must specify advisory, affected version/path, justification, compensating control, owner and expiry <=30 days; it cannot waive a known exploitable high/critical risk. Fix reachable critical issues before release; contain immediately if live. Keep operational incident details private, rotate exposed keys, reconcile finance/stock, and add a regression test.

## Reliability acceptance

Initial targets: commerce RPO <=15 minutes and RTO <=4 hours, validated with a restore rehearsal. Backups must include DB, private/public object references and required configuration, with keys stored separately. Monthly restore exercises and alert tests. Preserve financial/order history; rollback must not undo a successfully paid order. Test worker termination between send/commit/ack and queue recovery.

Before live sales, assign incident owner/on-call channel, validate alerts for payment reconciliation lag, stock drift, dead letters, checkout errors, storage pressure and backup failure. Use separate diagnostics and financial audit records. CI cannot prove carrier delivery or successful payout.
