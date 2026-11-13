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

Compose runs independent storefront, Rust API/worker, PostgreSQL and storage services. The current non-selling staging deployment uses the existing Traefik/Coolify TLS proxy; future production ingress needs its own reviewed policy. No public DB/admin debug port. Separate sandbox/staging/production secrets, profiles, sender and storage; least-privilege credentials with documented rotation and revocation. Inject secrets at runtime; verify logs/artifacts never disclose them. Access is audited and MFA protected. Use small bounded production-safe health endpoints without customer data.

Metrics/logs remain exportable to free self-hosted tools through open formats; add OpenTelemetry instrumentation and a modest Prometheus/Grafana or equivalent setup only for useful alerts. Track payment reconciliation age, stock exceptions, queue lag/dead letters, mail failures, error rate/latency, disk/DB connections and backup age. Define an owner and response for each alert. Do not add a paid monitoring dependency.

## Release gates

`npm run release:check` remains blocked while production is disabled. P7 requires application stage, `productionEnabled`, every configured production gate passed, and the server protection audit. The manifest is `.local/releases/<full-HEAD>/evidence.json`, with one entry for each and only each key in `config/delivery.json`'s `productionGates`. For example, with an illustrative two-gate configuration:

```json
{
  "version": 1,
  "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "gates": {
    "securityAssessment": {
      "gate": "securityAssessment",
      "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "status": "passed",
      "evidence": "security.txt",
      "sha256": "<lowercase SHA-256 of security.txt bytes>"
    },
    "restoreAndRollback": {
      "gate": "restoreAndRollback",
      "head": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "status": "passed",
      "evidence": "restore.txt",
      "sha256": "<lowercase SHA-256 of restore.txt bytes>"
    }
  }
}
```

Replace the illustrative head and digest placeholders with `git rev-parse HEAD` and `shasum -a 256` of each actual artifact. Paths are relative to the manifest's revision directory; use distinct nonblank regular files, with no symlinks, traversal, hard-link aliases, duplicate content, or reference to the manifest itself. Extra or missing fields and gates are rejected. Keep the manifest and evidence in ignored local storage or secured CI artifacts; do not include customer data in product docs. Then run `npm run release:check` from the reviewed revision. A new commit requires new revision-bound evidence and manifest.

The check reads delivery policy and application-file presence from the committed revision and requires the same clean source and index before and after evidence validation and the server audit. Only committed regular files count as application source. Untracked files also block it unless ignored; ignored local evidence remains usable. Keep source and evidence stable throughout validation: these checks do not prevent a concurrent writer from changing and restoring files between checks, or from changing an opened evidence file. The check verifies the declared binding and file integrity; it does not authenticate who produced evidence or judge its substance. Qualified humans must review each artifact and the actual provider, deployment, recovery and physical fulfilment acceptance before activation.

Before release: all required CI and independent reviews; populated Sonar with zero issues/hotspots; application/security/performance/discovery evidence; real market/provider/product/legal acceptance; isolated restore and compatible rollback. Create a release PR develop → main; no direct push. Build immutable digest-tagged images from the reviewed revision, sign/attest provenance, deploy staging and verify served version/health/routes before production. Add the deployment workflow only when these gates are real; merging foundation does not deploy anything.

Migrations follow expand/contract. Apply backward-compatible schema before switching services; test old/new code coexistence. Prove actual startup compatibility between each image's embedded migration manifest and the stored versions/checksums: an older SQLx migrator can reject newer history even when its queries still work. Irreversible data migrations need explicit backup/recovery design and approval. Roll back images only if that compatibility is proven; never delete migration history, reverse migrations or reset financial data to force an old image to start. Production requires its own compatible migration, coexistence and recovery plan. Reconcile provider outcomes after incidents before retrying financial effects.

The API executable accepts `migration-manifest` without configuration, filesystem
access or a database connection. Its JSON object has `schema_version: 1`, the
compiled SQLx `ignore_missing` policy and ordered `migrations` entries containing
a decimal-string `version`, `kind` (`simple`, `reversible_up` or
`reversible_down`) and lowercase SHA-384 `checksum_sha384`. Both members of a
reversible pair may have the same version. Extra arguments fail. Capture the
output from the exact image and bind it to a separately verified image digest;
the output does not attest its own provenance. Compare applied successful
`_sqlx_migrations` versions and checksums, report candidate-only forward
migrations as pending, and reject dirty records, checksum differences or applied
versions absent from a binary whose `ignore_missing` is false. A manifest is not
proof that pending migrations run, that queries remain compatible, or that an
image starts or can roll back. Older binaries may not provide this command.

For a quiescent non-selling preview only, the [staging runbook](../deploy/staging/README.md#rollback) permits a consistent snapshot restored into a separate owned project/database while preserving the old database and containers. Verify restore integrity, private startup and exclusive router ownership before switching. If business writes occur after the snapshot, require reviewed catch-up/reconciliation before cutover or rollback; a snapshot must not discard those writes.

## Recovery and acceptance

Proposed service targets: database RPO <=15 minutes and RTO <=4 hours, subject to a measured rehearsal and approved hosting capacity. Encrypted off-host DB/backups, WAL/PITR where needed, media backups and independently stored keys. Alert on age/failure; restore to a fresh isolated environment, verify row counts/constraints, representative orders/stock and private media, then measure elapsed time. A successful backup job is not restore evidence.

For a bounded local diagnostic, `P1_POSTGRES_BIN=/path/to/postgresql18/bin python3
scripts/rehearse_restore.py` creates two disposable loopback-only native clusters.
It uses the embedded project migrations and existing synthetic order, stock and
encrypted verification fixtures. A single exported source snapshot supplies both
the custom-format dump and full per-table row counts and canonical content hashes;
the restore must match, including SQLx migration history and checksums. The runner
also compares public sequence `last_value` and `is_called`, with the exercised
synthetic stock sequence as an altered-state negative control. PostgreSQL sequences
are not MVCC-snapshotted: this comparison relies on the exclusively owned source
having no other client writers. The runner checks for other clients and unchanged
sequence state around the dump. A held named extra client must be observed and
rejected, and its closure must restore quiescence. The runner cannot detect a
change reversed between checks.
It also tests that altered restored content and a corrupt archive are detected. Its redacted
output records source revision, clean-source checks, archive and comparison hashes,
tool version and elapsed time. Run it from a clean reviewed revision with a bounded
owned Cargo target and sufficient disk; stopped temporary clusters are removed, and
a cluster that cannot be stopped is retained for inspection. The comparison covers
public tables, migration rows and public sequence values, not complete schema,
constraint definitions or sequence configuration.
After comparing the restored tables, the synthetic fixture supplies separately
held, hardcoded test keys to the trusted in-process opening path. It verifies an
unclaimed restored envelope with its original key, rejects an incorrect key and
does not reopen a start already committed before the dump. It then registers a
distinct fresh synthetic key and issues and opens a new synthetic envelope in the
restored copy. Only the copy is modified by these checks. This is not a general
historical reader, mailbox delivery, actual key custody or automatic detection of
a restored database; operators must still provision the fresh active key.
This synthetic test does not exercise staging images, media, operational key rotation, shared
proxy or router switching, off-host backups, live data, provider recovery, or the
production RPO/RTO. It cannot by itself pass the restore and rollback release gate.

Use each verification delivery key in one database allocation domain. Reserve
nonces with `fsync=on` and synchronous commit before encrypting. After a restore,
clone, asynchronous replica promotion or uncertain rollback, provision a fresh
active encryption key before resuming issuance. Retain earlier keys only under
approved historical read and retention controls. Rehearse restoration with
synthetic ciphertext and separately stored keys before activation.
A delivery start with no provider outcome is ambiguous after interruption.
Keep it visible as unknown. Define provider idempotency or a reviewed recovery
policy before connecting transport or retrying any ambiguous delivery.

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

## Private paid-parent evidence inspection

In an isolated local/test environment, run
`commerce-worker --inspect-paid-parent <attempt-id> <effect-key>` for one exact
stored operation. It needs PostgreSQL and does not load a Mollie credential.
It reports absent, pending or complete and, for complete evidence, whether each
of the four totals is present. It emits neither the actual totals nor the
lookup identifiers. Pending without a receipt stays unresolved; complete
describes one historical GET. This command cannot fetch, retry, reconcile a
balance, or authorize a refund.

## Private reversal-page supervision

In an isolated local/test environment, `commerce-worker --enroll-reversal-scan
<attempt-id> <paid-observation-id> <refund|chargeback> <effect-key>` verifies the
configured dedicated test profile, then enrolls an authenticated paid observation.
The two IDs are internal 32-character lowercase hexadecimal identifiers; the
stable effect key is 1–128 ASCII letters, digits or hyphens. Reuse the same key
only for the same intent. The command prints the internal job ID for follow-up.
It does not run the scan itself.

`commerce-worker --reconcile-reversal-pages` verifies the profile once and runs
one bounded pass; `REVERSAL_PAGE_BATCH` defaults to 10 and accepts 1–25 steps.
Both commands require the same dedicated `MOLLIE_TEST_API_KEY` and
`MOLLIE_TEST_PROFILE_ID` as payment reconciliation. Production and live keys are
rejected. `commerce-worker --inspect-reversal-scan <job-id>` uses PostgreSQL to
report validated state, generation, failure count and review reason. Provider
credentials and provider network access are unnecessary for inspection. Batch
output reports the number of processed steps. Inspect the enrolled job and its
private evidence for review stops or retries. An ended or capped scan leaves
provider history unresolved. Review recovery, scheduling, financial and
inventory transitions, provider sandbox acceptance and deployment remain open.

## Private reversal-item supervision

In an isolated local/test environment, `commerce-worker --enroll-reversal-item
<attempt-id> <paid-observation-id> <page-id> <ordinal> <effect-key>` binds a
candidate position to a verified page and paid observation. The ordinal is
one-based and between 1 and 10. The configured test key must match the page's
authenticated fingerprint. Enrollment returns an internal job ID.

`commerce-worker --reconcile-reversal-items` verifies the configured test
profile and runs one bounded pass. `REVERSAL_ITEM_BATCH` defaults to 10 and
accepts 1–25 steps. `commerce-worker --inspect-reversal-item <job-id>` reads
validated state from PostgreSQL without provider credentials. An observed job
retains one authenticated fact. Inspection of an uncertain job indicates when
the operation's exact late observation is available. The review state and
provider history remain unresolved.

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

## Disabled staging callback edge

`commerce-callback-edge` is a separate Rust process with no database or provider
credentials. It is disabled by default and rejects production. Its configured
exact proxy peers come from actual socket metadata, rather than request headers.
The normal API's loopback Bearer gate and Next's browser POST gate stay intact.

The optional [staging overlay](../deploy/staging/callback-edge.compose.yml) starts
only when explicitly included with its profile and required configuration. Starting
the private edge and enabling its Traefik ingress are separate actions;
`STAGING_CALLBACK_INGRESS_ENABLED` defaults false. Keep it false until the runbook's
runtime gates pass. No preview-token or provider credential belongs in labels.

Application limits are 16 inflight callbacks, 1024 body bytes within one second,
1500 ms for the complete fixed loopback API exchange and three seconds overall.
Only a finished empty upstream 200 becomes an empty external 200. No upstream
redirect, automatic POST retry, body, cookie or diagnostic header is relayed.
Health indicates process availability, not API/profile or provider readiness.

Verify the installed proxy version, unique peer-address mapping, address stability,
no direct-network bypass, actual middleware behavior and nonzero ingress upload
read timeouts before public activation. Traefik buffers client bodies before
Rust's body deadline begins. A shared SNAT gateway or Coolify subnet cannot stand
in for proxy identity. Then require exact-revision TLS and isolated provider
delivery acceptance. The committed source and Compose checks prove none of these
runtime states. Rollback disables both callback ingress and the separate web
ingress flag before recreating only this project's services.
