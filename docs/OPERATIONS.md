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

Keep all CodeQL action steps on the same commit. Dependabot groups their version
updates; review the resulting pins together and require fresh populated analysis
for each configured language before merging.

CI pins checkout 7.0.1, setup-node 7.1.0 and download-artifact 8.0.2 to
upstream commits. Checkout keeps credential persistence disabled. Node setup
uses the repository version and npm cache. The Sonar job downloads the
revision-named application artifact with digest verification enforced.

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

For an offline preliminary review of a Debian image vulnerability scan, use
`scripts/inspect_image_scan.py`. Keep the raw Trivy JSON with all packages listed,
the exact single-platform OCI manifest and its config bytes, and a separate capture
receipt. Do not substitute a finding-count summary for the raw report. The receipt
has these exact fields (replace the explanatory strings with actual values):

```json
{
  "schema_version": 1,
  "source_revision": "40-character lowercase commit ID",
  "manifest_digest": "sha256:platform-manifest-digest",
  "scanner_digest": "sha256:scanner-image-digest",
  "report_digest": "sha256:raw-report-bytes-digest",
  "scanned_at": "2026-11-15T11:00:00Z",
  "exit_code": 0,
  "database": {
    "version": 2,
    "updated_at": "2026-11-15T10:00:00Z",
    "next_update": "2026-11-16T10:00:00Z"
  },
  "scope": {
    "scanners": ["vuln"],
    "package_types": ["os", "library"],
    "severities": ["UNKNOWN", "LOW", "MEDIUM", "HIGH", "CRITICAL"],
    "include_unfixed": true,
    "include_all_packages": true,
    "show_suppressed": true,
    "ignore_rules": false,
    "vex": false
  }
}
```

With pinned Trivy v0.70.0, a trusted capture must use a controlled empty config,
empty ignore file and environment without `TRIVY_*` overrides. For the exact
platform image, the relevant unfiltered invocation is:

```sh
trivy --config CONTROLLED_EMPTY_CONFIG image --format json --scanners vuln \
  --pkg-types os,library --severity UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL \
  --pkg-relationships unknown,root,workspace,direct,indirect \
  --list-all-pkgs --show-suppressed --exit-code 0 \
  --ignorefile CONTROLLED_EMPTY_IGNORE_FILE --platform linux/amd64 \
  --output RAW_TRIVY.json IMAGE_AT_VERIFIED_PLATFORM_DIGEST
```

Do not use ignore-status, ignore-unfixed, ignore-policy, VEX, skipped files or
directories, alternate package relationship filters, or other settings that omit
findings or targets. Preserve the capture's effective settings and scanner and
database provenance for independent review. The receipt's `show_suppressed` field
attests that the flag was enabled; absent modified findings alone cannot prove it,
because Trivy's [JSON writer](https://github.com/aquasecurity/trivy/blob/v0.70.0/pkg/report/json.go)
otherwise removes them. `--show-suppressed` does not authenticate capture or detect
every way a scan may have been filtered.

Use RFC 3339 UTC timestamps from the capture and vulnerability database metadata,
and run the audit with independently verified source revision, scanner image digest,
platform manifest digest, and platform:

```sh
python3 scripts/inspect_image_scan.py --receipt PRIVATE_RECEIPT.json \
  --manifest OCI_PLATFORM_MANIFEST.json --image-config OCI_CONFIG.json \
  --report RAW_TRIVY.json --revision REVIEWED_COMMIT \
  --image-digest sha256:VERIFIED_PLATFORM_MANIFEST_DIGEST \
  --scanner-digest sha256:VERIFIED_SCANNER_IMAGE_DIGEST --platform linux/amd64
```

The audit requires a populated Debian inventory, matching manifest/config and scan
identity, a successful unfiltered scope and a database valid both at scan time and
at audit time. It rejects modified findings, malformed evidence, stale database
metadata and any reported HIGH or CRITICAL finding. Its zero finding result means
only that none were reported in the supplied evidence. Scope, scanner execution,
database metadata, source/build provenance and artifact custody must be established
by trusted capture and independent review; self-declared receipt fields cannot prove
them. The audit provides no waivers and does not replace release evidence or human
security assessment. Review other severities, package reachability, SBOM, provenance
and runtime separately. See the [Trivy report schema](https://github.com/aquasecurity/trivy/blob/v0.70.0/pkg/types/report.go)
and [vulnerability scan configuration](https://trivy.dev/docs/v0.70/guide/scanner/vulnerability/).
The focused tests include a field-level control based on the pinned upstream
[Debian golden](https://github.com/aquasecurity/trivy/blob/v0.70.0/integration/testdata/debian-buster.json.golden)
and [language golden](https://github.com/aquasecurity/trivy/blob/v0.70.0/integration/testdata/busybox-with-lockfile.json.golden).
Those goldens omit package inventories, so they are not full compatibility proof
for a deployed image or this capture contract. A missing language result may mean
no recognizable language artifact was found; it does not prove none exists.

Migrations follow expand/contract. Apply backward-compatible schema before switching services; test old/new code coexistence. Prove actual startup compatibility between each image's embedded migration manifest and the stored versions/checksums: an older SQLx migrator can reject newer history even when its queries still work. Irreversible data migrations need explicit backup/recovery design and approval. Roll back images only if that compatibility is proven; never delete migration history, reverse migrations or reset financial data to force an old image to start. Production requires its own compatible migration, coexistence and recovery plan. Reconcile provider outcomes after incidents before retrying financial effects.

The API executable accepts `migration-manifest` without configuration, filesystem
access or a database connection. Its JSON object has `schema_version: 1`, the
compiled SQLx `ignore_missing` policy and ordered `migrations` entries containing
a decimal-string `version`, `kind` (`simple`, `reversible_up` or
`reversible_down`) and lowercase SHA-384 `checksum_sha384`. Both members of a
reversible pair may have the same version. Extra arguments fail. Capture the
output from the exact image and bind it to a separately verified image digest.
Capture database history with one read-only query against the selected copy.
Use the same database, connection role and effective `search_path` as the
application. The unqualified table name resolves the same way as it does for
the embedded migrator. Keep connection details out of the artifact. An empty
migration table produces an empty history, while a missing table fails the capture.
For the opted-in preapplied preview, set `pg_catalog, public, pg_temp` in the
read-only capture transaction to match the API pool's fixed search path.

```sh
umask 077
psql -X -qAt -v ON_ERROR_STOP=1 > history.json <<'SQL'
BEGIN READ ONLY;
SELECT json_build_object(
  'schema_version', 1,
  'migrations', COALESCE(json_agg(json_build_object(
    'version', version::text,
    'checksum_sha384', encode(checksum, 'hex'),
    'success', success
  ) ORDER BY version), '[]'::json)
)
FROM _sqlx_migrations;
COMMIT;
SQL
python3 scripts/check_migration_compatibility.py candidate-manifest.json history.json
```

The offline comparison rejects dirty or duplicate rows, checksum differences
and missing applied versions when `ignore_missing` is false. It lists pending
forward versions and any applied versions permitted by `ignore_missing`. The
report records SHA-256 digests of both captured files. After applying candidate
migrations to an isolated copy, recapture that copy's history and compare it
with both binaries. Add `--require-applied` to check the preview's startup
requirements for a strict manifest and complete history. Retain the image
digest and database
snapshot identity alongside the report. Prove image provenance, migration
execution, query compatibility and actual startup separately. Older binaries
may lack the manifest command.

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
Apply the same single allocation-domain and fresh-key-after-restore rules to
the purpose-derived guest-link delivery key. A nonce prefix and HKDF purpose
separation do not make rollback of the same key's counter safe. The new key
has no configured runtime provisioner or delivery transport.
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

### Restricted preview database rehearsal

The catalogue lock-helper fixture describes an offline observer, not operational
provisioning. Before preparing any target grants, identify the exact reviewed
source and image digest, embedded migration-manifest digest, intended database
and snapshot, migration owner, runtime LOGIN and separate helper NOLOGIN role.
Keep credentials and connection URLs outside the grant review and evidence. A
qualified database operator must verify effective grants, PUBLIC permissions,
role memberships, function ownership and schema ownership on the selected copy;
a role name or a clean source checkout does not prove those properties.

For the synthetic observer, the fixture grants SELECT on `product_families`,
`physical_components`, `catalogue_items`, `catalogue_variants`, `catalogue_boms`,
`catalogue_bom_lines`, `catalogue_definition_events`, `catalogue_factual_reviews`,
`catalogue_family_routes`, `catalogue_image_renditions`,
`catalogue_image_reviews`, `catalogue_media_assets`, `catalogue_media_rights`,
`catalogue_media_serialization`, `catalogue_price_revisions`,
`catalogue_publication_revisions`, `catalogue_specification_revisions`,
`catalogue_translation_reviews`, `catalogue_translation_revisions`, and
`stock_balances`. On `catalogue_image_originals` it grants only `asset_id`,
`recipe`, `sha256`, `actor`, `display_width_px` and `display_height_px`; it does
not grant `bytes` or table SELECT. It adds column SELECT on `_sqlx_migrations` for `version`,
`success` and `checksum`, and on `catalogue_translations` for `sku`, `locale`,
`title`, `description` and `care`. These are the fixture's exercised grants,
not a proof that each table-level SELECT is individually minimal. Invoker
functions used by the published read also need their effective EXECUTE and
underlying read privileges checked.

The helper owner has schema USAGE and only identifier SELECT/UPDATE on
`catalogue_variants.sku` and `catalogue_media_serialization.asset_id` in the
fixture. The observer has EXECUTE on both `claim_published_variants(text[])`
and `claim_published_assets(text[])`, no helper-role membership, no schema
CREATE and no direct row-update authority. The migration owner applies schema
separately; preapplied startup only observes migration history. PostgreSQL
requires a normal transaction for the helper's row locks even with this
restricted observer. Ownership transfer and grants require independently
verified operator authority; this fixture does not create any target roles.

A separate non-login integrity-helper owner needs only `asset_id`, `recipe`,
`sha256` and `bytes` SELECT on originals; the observer needs only EXECUTE on
`preview_original_intact(text)` and must not inherit that owner. The helper hashes
one bounded original and returns a boolean, not its bytes or a rights decision.
Migration 0078 creates no roles and grants no caller access; a role with direct
original-byte access keeps the legacy path, while a restricted observer needs
the explicit helper grant. Check effective PUBLIC and inherited privileges, the
function owner, locked search path and schema before any real provisioning.
The isolated integrity controls exercise ordinary-schema SHA and byte-comparison
operator shadows under a hostile search path, including damaged and repaired
originals and renditions.
The helper exposes an integrity/existence result to its grantee. Hashing can read
20 MiB per asset, up to 25 candidates per page, and image delivery rechecks the
publication after decode. Benchmark real resource limits and repeated requests;
the statement timeout alone is not a total workload bound. This rehearsal uses
synthetic data and demonstrates no actual startup, HTTP delivery, approved
photographs or staging deployment.

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

An operator with access to the isolated database can run
`commerce-worker --inspect-payment-recovery <attempt-id>` to inspect one stored
checkpoint in local/test mode. The command uses no provider credential or
network call to Mollie. It reports either an absent checkpoint or the current
phase, review reason, generation, resume revision, deadline and retry counters.
It omits the attempt, provider, outbox and page identifiers. An absent checkpoint
means there is no job row for the supplied attempt. Review the linked immutable
search, observation and resolution receipts before an operational decision.
A separate `commerce-worker --inspect-payment-custody <attempt-id>` validates one
original frozen attempt, dispatch and exact recovery event in a read-only snapshot,
even before a job exists. It reports a checkpoint's closed state and linked-page
presence, observation and distinct identity counts, whether paid was observed, and
whether the latest stored resolution covers the retained prefix. Claimed consumed
candidates require matching authenticated observations, and polling requires a
compatible historical supervisor resolution; later observations remain visible.
These checks do not establish the supervisor's exact execution. It checks only the
checkpoint-linked search chain; unlinked pages and external reports are not included.
Zero identities do not prove provider absence. It emits no identifiers or credentials,
creates no job and never contacts Mollie. This inspection does not establish source,
database or image provenance, schema compatibility, payment status or permission to
run recovery. Check those independently; preserve the original custody and slot.

Review stops retain unresolved, ambiguity, paid-observed, scan/poll/step-limit,
custody-invalid or exhausted-retry reasons. Any stock or financial transition
requires its own current evidence and authority. Preserve an uncertain provider
outcome after an empty search. The private finance-authorized resume operation
handles eligible transient stops with a bounded, audited decision.

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

`commerce-worker --inspect-paid-parent-set <attempt-id>` inspects the bounded
stored set under the attempt lock without provider credentials. It emits only
operation, pending and complete counts and a deterministic SHA-256 digest bound
to the current received recognition and all validated operation/receipt states.
The digest is a detached snapshot: re-inspect after any new observation, start or
late receipt. It does not attest provider-history completeness or authorize a
financial action. A stale or damaged source fails closed.

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
report the stored state, generation and review reason after source validation.
For states other than an uncertain stop it also reports the stored failure count. Provider
credentials and provider network access are unnecessary for inspection. Batch
output reports the number of processed steps. For an uncertain stop, inspection
also reports whether that operation's validated late page is present; other
states report `not_applicable`. For uncertain stops it omits the failure count:
inspection does not reconstruct prior operations. It does not advance the scan or prove provider
history complete. Inspect the enrolled job and its
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

For an authorized local/test staff session, `commerce-worker
--review-reversal-inbox` reads one current review stop without provider credentials.
Supply exactly 32 raw session bytes followed by EOF on non-terminal stdin within
five seconds; never place the session in an argument. The closed output is empty
or contains the internal job ID and update time with a review summary. Continue
with `--review-reversal-inbox <updated-at> <job-id>` using those exact values.
This is a live scan; restart without a cursor to find earlier changes. It cannot
retry, fetch from the provider, resolve the stop or authorize financial effects.
Staff sessions and production operator access are not installed by this command.

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

### Hosted PostgreSQL scan capture

The narrowly scoped `PostgreSQL image evidence` workflow can capture the pinned
staging PostgreSQL image from Docker Hub on a disposable GitHub-hosted runner.
It is triggered by relevant pull request paths or by manual dispatch. Its checkout
is bound to the pull request head or dispatched commit; Compose renders the checked
in configuration without interpolation, and the capture verifies the `db` service's
staging index, its Linux AMD64 platform manifest and image config. The
scanner is Trivy 0.70.0 at its pinned Linux AMD64 image digest and accesses the
registry directly without the Docker socket or credentials. Empty configuration
and ignore files and a fresh scanner cache are used for each run.

The artifact contains the raw report, image and scanner identity documents, empty
configuration, database metadata, command, scanner log, receipt, offline audit
output and checksums; the database cache is not uploaded. The offline audit must
accept the evidence's structure even when its result is blocked by findings.
Scanner errors fail capture, while reported vulnerabilities do not fail capture
or constitute an acceptance. Independently verify the workflow revision, scanner
and image identities, scope and custody; inspect the unfiltered report with the
offline audit above. A successful capture does not establish deployment, image
provenance, vulnerability disposition or release readiness.
