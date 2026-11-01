# Operations and release

## Repository and dependency ownership

Develop is integration; main is production releases only. Required contexts live in `config/delivery.json` and `config/github-ruleset.json`. Change workflow, local gates, tests and live rules together. Run `python3 scripts/audit_github.py`; a JSON file alone does not prove enforcement. Protect both branches, no bypass actors/direct push/deletion/force push, current base, resolved review threads and every check successful.

The repository is public and the protection audit verifies its server rules. CI evidence uploads remain mandatory: a storage quota failure blocks acceptance even if tests/builds completed first, and browser/Sonar acceptance cannot be inferred from that partial run. Preserve existing evidence rather than deleting it to hide a blocker.

On GitHub Free, a private organization repository does not retain the former public server protection/native security availability. Restore an eligible protected setup before merging: obtain the owner's explicit decision before restoring public visibility, establish and verify an equivalent no-bypass private workflow on a Piwas-controlled free forge/runner (for example, self-hosted Forgejo with branch rules, required statuses and isolated runners), or explicitly choose a GitHub plan that supplies those controls. Do not buy a plan or migrate automatically. Adapt and test the merge/audit tooling before a move; local hooks alone are insufficient. Sonar Community, Gitleaks, Rust/TS checks and tests remain self-hostable without paid subscriptions; GitHub-specific CodeQL/native secret scanning need an available alternative on the target platform. Production remains blocked while enforcement is unavailable.

For isolated local/test inventory work, the default `commerce-worker` command is
a read-only dependency probe. The explicit `commerce-worker --expire-holds`
command performs one bounded pass; `HOLD_EXPIRY_BATCH` is 1–100, default 25. It uses
only that environment's database and rejects production. Failed holds retain
redacted retry metadata and return a nonzero process status; a fresh pass retries
eligible failures after 60 seconds. Log output includes counts, never owner IDs,
database URLs or raw SQL errors. Do not configure production supervision until P7
environment, restore/rollback and operational gates pass. The command is not a
payment reconciler and cannot consume or reallocate a late paid order.

Use only reviewed free self-hostable libraries/tools; retain licences/notices and produce an SBOM for release images. Exact direct dependencies and committed lockfiles; actions commit-pinned and images digest-pinned. Weekly update PRs, prompt security updates, no automatic major upgrades. Rebuild reproducibly, run contract/migration/regression checks and retain image provenance. Reject unknown/restrictive licences until reviewed; no dependency that requires a paid service to keep checkout working.

## Environments and credentials

Compose runs independent storefront, Rust API/worker, PostgreSQL and storage services behind Caddy. No public DB/admin debug port. Separate sandbox/staging/production secrets, profiles, sender and storage; least-privilege credentials with documented rotation and revocation. Inject secrets at runtime; verify logs/artifacts never disclose them. Access is audited and MFA protected. Use small bounded production-safe health endpoints without customer data.

Metrics/logs remain exportable to free self-hosted tools through open formats; add OpenTelemetry instrumentation and a modest Prometheus/Grafana or equivalent setup only for useful alerts. Track payment reconciliation age, stock exceptions, queue lag/dead letters, mail failures, error rate/latency, disk/DB connections and backup age. Define an owner and response for each alert. Do not add a paid monitoring dependency.

## Release gates

`npm run release:check` intentionally fails during foundation. P7 requires application stage, productionEnabled and evidence for every production gate bound to the exact release SHA. Evidence lives in ignored local/secured CI artifacts; product docs contain current readiness, not customer data or session history.

Before release: all required CI and independent reviews; populated Sonar with zero issues/hotspots; application/security/performance/discovery evidence; real market/provider/product/legal acceptance; isolated restore and compatible rollback. Create a release PR develop → main; no direct push. Build immutable digest-tagged images from the reviewed revision, sign/attest provenance, deploy staging and verify served version/health/routes before production. Add the deployment workflow only when these gates are real; merging foundation does not deploy anything.

Migrations follow expand/contract. Apply backward-compatible schema before switching services; test old/new code coexistence. Irreversible data migrations need explicit backup/recovery design and approval. Roll back images only if schema compatibility is proven; financial data is never reset to undo a deployment. Reconcile provider outcomes after incidents before retrying financial effects.

## Recovery and acceptance

Proposed service targets: database RPO <=15 minutes and RTO <=4 hours, subject to a measured rehearsal and approved hosting capacity. Encrypted off-host DB/backups, WAL/PITR where needed, media backups and independently stored keys. Alert on age/failure; restore to a fresh isolated environment, verify row counts/constraints, representative orders/stock and private media, then measure elapsed time. A successful backup job is not restore evidence.

Incident runbook: contain credentials/access, preserve redacted evidence, stop unsafe fulfilment/payment retries, establish last known good state, reconcile outstanding attempts, restore/roll forward, verify and communicate through approved channels. Rotate exposed keys with provider-specific steps; test revocation and rollback. Retention/deletion apply to logs and backups under the approved privacy policy.

Final operational acceptance checks real emails, sandbox methods plus separately authorized live payment/refund, warehouse pick/pack/carrier/return rehearsal, correct VAT/invoice/credit documents, all intended markets, public crawl/index policies and device accessibility. Report what was actually measured; passing source tests does not prove delivery, payout, parcel fit or indexing.

## Non-selling staging preview

See [the staging runbook](../deploy/staging/README.md) for the dedicated HTTPS
preview on Piwas infrastructure. It is independent of the existing applications
and uses a separate database/volume, service names and generated credentials.
The storefront can be made publicly viewable with the explicit staging setting;
admin remains behind separate test-admin credentials and has no operational
capabilities. Production remains disabled. The empty migrated preview is useful for integration
wiring; no real merchant facts, tax/carrier readiness, staff authority, order flow
or provider acceptance is established by its deployment. Record accepted source,
image identity, TLS/auth/noindex/RTL smoke and rollback separately from CI.

## Supervised test-payment reconciliation

`cargo run --locked -p commerce-worker -- --reconcile-payments` is an explicit
one-pass local/test command. The default worker remains a read-only probe and every
command rejects production. Provide the dedicated `MOLLIE_TEST_PROFILE_ID` and
`MOLLIE_TEST_API_KEY` through the private environment; never reuse Sofra credentials.
Only `test_` keys and an authenticated matching profile are accepted. No key is
installed or provider call performed by adding this source. Keep secrets outside
shell history, source, logs and handoffs. `PAYMENT_RECONCILIATION_BATCH` defaults
to 10 and accepts 1–25 leased steps; one step makes at most one GET.

Each pass reports all outstanding review stops and failure/backoff jobs for the
verified profile, including unchanged earlier exceptions. Later idle passes remain
nonzero while that condition persists; ordinary pending-status polls are separate.

An operator must inspect the private `read_job` result and the linked immutable
search/observation/resolution receipts when the command reports review or retries.
Review stops retain unresolved, ambiguity, paid-observed, scan/poll/step-limit,
custody-invalid or exhausted-retry reasons. Paid-observed is not permission to
fulfil, consume/reallocate stock or refund; those transitions remain unimplemented.
An empty search is not absence, and no recovery result permits another create.
There is deliberately no automatic restart/requeue of a review stop. Add bounded,
authorized and audited operator recovery before introducing that ability.

For a process crash, wait for the 60-second lease to expire and run a later bounded
pass; existing page receipts resume without replaying a list operation. Repeated
fetches use new immutable facts and cannot clear paid evidence. Profile-scoped
jobs back off transient provider failures and stop visibly after five failures;
polls stop after ten attempts, searches after 100 pages, and jobs after 1,500 claims.
Do not install a scheduler or production supervision until launch ownership,
callbacks, financial transitions, late-paid stock recovery and provider acceptance
have separate evidence. Unit/loopback PostgreSQL tests are source evidence, not an
actual Mollie sandbox payment or deployed/physical acceptance.

## Private callback supervision

`cargo run --locked -p commerce-worker -- --reconcile-callbacks` runs one bounded
local/test pass. It uses the dedicated `MOLLIE_TEST_PROFILE_ID` and
`MOLLIE_TEST_API_KEY` policy described above; production and live credentials are
rejected. `CALLBACK_RECONCILIATION_BATCH` defaults to 10 and accepts 1–25 steps.
No credential, scheduler or public receiver is installed by this command.

The pass drains saved receipt work independently of current job leases, claims
at most one authenticated GET per generation and retains five lifetime generations
per intake. Expired or cancelled work cannot restore a single-use fetch permit.
Committed receipts are recovered before a fresh GET; transient failures back off
and stop visibly. A stale generation can preserve its fetched fact but cannot
advance a newer checkpoint. Delayed paid facts remain visible even after newer
open evidence or a review stop. Such facts never authorize shipment, stock
consumption/reallocation or refunds.

Review and retry counts include unchanged older exceptions. An intake retains
active capacity until every generation is processed without a review outcome;
an unresolved older GET cannot disappear behind a completed newer checkpoint.
Inspect the private job and its immutable generation, receipt, observation,
resolution and processing records. No automatic restart of a review stop exists.
If proof behind an existing processing result is damaged, inspect its separate
immutable quarantine record as well. The original result and payment facts remain
intact; exact proof repair alone does not clear quarantine or release capacity.
Public form ingress, edge controls, actual provider acceptance and deployment
remain separate gates.

The optional private HTTP intake uses `CALLBACK_INTAKE_PROFILE_ID` in the API
process. Its absence leaves `/api/callbacks/mollie` unregistered. A configured
profile enables only the existing Bearer-protected, loopback API route; it does
not change the staging browser gate. No provider key is needed in this process.
Use the same dedicated Test profile in the separately configured worker.

The route acknowledges an admitted classic form with an empty `200 OK` after
durable commit. This acknowledges custody, rather than a verified payment status.
Malformed input has no admission effect; quota, storage and request-timeout
failures remain non-successful. If an acknowledgement is lost after commit, a
repeated notification may create another bounded intake. The worker's receipt
and observation idempotency rules still govern payment facts. Public edge policy,
TLS/provider acceptance and deployment are separate work.
