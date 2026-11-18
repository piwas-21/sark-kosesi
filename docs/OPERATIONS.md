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

## Private packing inspection

In an isolated local/test environment, `commerce-worker --inspect-packing
<order-id>` validates the order's recorded packing assertions. `DATABASE_URL`
points to the isolated database. The command requires a current `warehouse.manage`
staff session with recent MFA. Supply exactly 32 raw session bytes followed by EOF
on non-terminal stdin within five seconds. Keep the credential out of arguments
and logs. The command returns one JSON object on stdout with `kind` set to
`packing_assertions`, per-SKU counts in `lines` and recorded actions in `history`.
Store this output as private order evidence. Assertion counts do not establish
which units have shipped or whether dispatch is permitted.

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
staging PostgreSQL image through the public Docker Hub mirror `mirror.gcr.io` on a
disposable GitHub-hosted runner. All retrieved image identity documents are checked
against their pinned digests before use; mirror availability on a runner is not assumed.
It is triggered by relevant pull request paths or by manual dispatch. Its checkout
is bound to the pull request head or dispatched commit; Compose renders the checked
in configuration without interpolation, and the capture verifies the `db` service's
staging index, its Linux AMD64 platform manifest and image config. The
scanner is Trivy 0.70.0 at its pinned Linux AMD64 image digest and accesses the
registry directly without the Docker socket or credentials. Empty configuration
and ignore files and a fresh scanner cache are used for each run. The scanner
uses the unprivileged runner's UID and GID to write into owner-only scratch.

The artifact contains the raw report, image and scanner identity documents, empty
configuration, database metadata, command, scanner log, receipt, offline audit
output and checksums; the database cache is not uploaded. The offline audit must
accept the evidence's structure even when its result is blocked by findings.
Scanner errors fail capture, while reported vulnerabilities do not fail capture
or constitute an acceptance. A sanitized capture status is retained even if an
identity request fails; it records the mirror endpoint and HTTP status without
credentials or response bodies. Independently verify the workflow revision, scanner
and image identities, scope and custody; inspect the unfiltered report with the
offline audit above. A successful capture does not establish deployment, image
provenance, vulnerability disposition or release readiness.

## PostgreSQL sibling root bootstrap

The path-scoped `PostgreSQL sibling runtime root` workflow builds a disposable
AMD64 Debian root without PostgreSQL. Its digest-pinned Docker Official Debian
root shares the pinned stock PostgreSQL image’s first layer; the workflow
checks registry identities, package inventory against the upstream root manifest,
and the actual stock image. A scoped, signed Debian snapshot supplies exactly
three pinned ICU and locale packages, and the resulting package inventory must
otherwise match the original root. The original util-linux packages remain.
The workflow records raw package names, versions and dpkg status; only fully
installed packages count toward closure, and partial or broken states fail.
It also records loader caches, OCI image
metadata, matching libc bytes and a small locale comparison, with positive and
negative controls for selected stock-only packages and libraries. Trust inputs,
source pins and snapshot provenance are in `ops/db-sibling-runtime-root/`.

This is a bootstrap and closure experiment, not a PostgreSQL runtime or an image
security scan. It starts no database, provides no deployment entrypoint and does
not establish PostgreSQL client dependencies, collation equivalence, operational
readiness or vulnerability disposition. A separate reviewed slice must add the
verified PostgreSQL install, exercise the runtime and compare unfiltered scans.
No image is published or deployed by this workflow.

## PostgreSQL sibling composition lab

The path-scoped `PostgreSQL sibling composition lab` rebuilds and validates the
sibling root and source feature experiments at the same checkout. It installs
only the snapshot-pinned readline runtime pair and the already-present terminal
library version, then copies the source experiment’s `/opt/pg-smoke` tree into
the sibling root. Package delta, file hashes, modes, owners and symlinks are
compared. The lab inspects executable and module ELF linkages and samples
libraries actually loaded by synthetic PostgreSQL and psql processes; missing
or selected prohibited libraries fail. These measurements do not cover every
possible dynamically loaded extension or operating path.

Network-isolated, non-root disposable clusters exercise SCRAM success and a
specific wrong-password rejection, a stock-to-composed compressed custom-format
restore and an invalid archive control. The source tree, exact package versions,
image identities, fixture outcomes and trust evidence remain in the workflow
artifact. This is a feasibility test, not a bit-reproducible build, a complete
entrypoint or lifecycle test, a TLS test, a vulnerability scan, staging clone or
rollback acceptance. It publishes no image and does not deploy or alter staging.

## PostgreSQL sibling scan comparison

The path-scoped `PostgreSQL sibling scan evidence` workflow first rebuilds and
checks the source, sibling root and composition on one disposable AMD64 runner.
It validates the saved composed image’s configuration and all layer hashes,
then compares the pinned stock image and that exact composed image with the
pinned Trivy scanner and one freshly acquired, unchanged vulnerability database.
Both raw reports retain every reported severity, status and package inventory;
CycloneDX files are converted from those same reports. A stock positive finding,
image identities, report and database timestamps, dpkg inventories and empty
suppression inputs are checked. Exact unmatched scanner rows are recorded as
comparison evidence, not as vulnerability fixes or acceptance.

PostgreSQL and libpq in the composed image are compiled from source rather than
installed as Debian PostgreSQL packages. The package scanner and generated SBOM
may omit those components and their advisories. A separate same-run source
component manifest records the upstream version and archive digest, executable
hashes and evidence limits; it does not invent scanner matches or establish
source vulnerability status. A source-aware advisory review remains necessary.
All scanner findings remain open until separately disposed; scanner output is not
proof of complete vulnerability coverage, deployment, or release readiness.
The workflow publishes no image and does not connect to staging data.

## Isolated PostgreSQL zlib candidate

`ops/db-zlib-candidate/Dockerfile` and `scripts/test_db_zlib_candidate.sh`
prepare a disposable AMD64-only candidate from the pinned PostgreSQL platform
manifest. The candidate obtains one exact `zlib1g` security package from the
Debian security archive using the pinned base's Debian archive keyring, checks
APT's authenticated index and a pinned package SHA-256, and rejects any change
to another installed package. A read-only build mount provides the hosted
runner's CA bundle for HTTPS; APT signature verification remains mandatory.
It preserves the signed APT index, exact package, trust-input hashes, bounded
stage results and before/after inventories in the candidate evidence. Raw
package-manager output is not retained or printed. Treat those records as
review evidence, not independent attestation by themselves.

The path-scoped PostgreSQL zlib candidate workflow runs the script on a
throwaway Linux runner when its candidate files change in a PR. It dumps
synthetic rows from the pinned Debian base and restores a compressed
custom-format archive on the candidate. Both database containers run without
external networks, and the lab checks an invalid archive rejection.
It scans the pinned base and built candidate using the same fresh vulnerability
database, retains both raw reports, and checks the target zlib package and
finding delta. Success establishes only the checks actually recorded. Review
image and package provenance, other vulnerabilities, restore and rollback,
operational configuration and exact-revision release gates separately. The
workflow does not update staging, activate production or accept any risk.

## Combined Debian candidate comparison

The hosted Debian candidate workflow builds the same pinned PostgreSQL base
with the authenticated zlib package and the independently rebuilt Gosu 1.20
executable. The builder checks the baseline Debian PostgreSQL uid, gid and
supplementary groups, the non-root privilege denial, and the oversized group
control. The combined build verifies both binary builds and uses the original
zlib Dockerfile, then adds the exact Gosu binary as one layer. The saved image
check verifies the base layer lineage and preserved runtime metadata.

One fresh Trivy database is used for both stock and combined image scans. The
comparison checks exact image metadata, package versions and positive finding
controls for the targeted zlib and Gosu advisories. Raw inventories and every
finding delta stay in the artifact. Release review still requires separate
disposition of residual or new findings.

An isolated base-to-candidate restore must preserve default and explicit C
collation probes, the synthetic stock receipt read and idempotent replay,
table and sequence state, and preparation of a schema-66 copy for migration to
the current revision. The fixture includes restore and rollback controls. Actual
staging data, security disposition and operational rollback still need review
on a separate isolated copy before an image switch.

## Optional PostgreSQL Gosu rebuild experiment

The `PostgreSQL Gosu candidate` workflow uses a disposable runner and synthetic
data. It pins Gosu 1.19's upstream source
commit and source-archive SHA-256, the official Go 1.27.4 Linux AMD64 archive
SHA-256, and dependencies against the source's `go.sum`. The archive hash fixes
what the experiment builds; the source commit is not signed, and neither this
pin nor a matching local build proves correspondence to the maintainer's signed
release binary. Source and Go archives are fetched only on the hosted runner.

After dependencies are prepared, two fresh, separate containers build the same
read-only source and vendor inputs with network disabled. The byte comparison
proves only repeatability under those inputs and this runner; it is not independent
provenance. The artifact records the executable digest, `go version -m`, vendor
file hashes, candidate image identity and synthetic checks. The pinned PostgreSQL
platform is used only as an isolated test image; the candidate is not published or
installed. One test checks PostgreSQL UID/GID and compares its non-root group set
with the pinned base, rejects a non-root attempt to become root, checks the selected Gosu path and bytes,
and starts an empty PostgreSQL instance for a synthetic query without network
access. It does not cover every identity, group, signal, ownership or restore case.

The workflow runs for pull requests that change its candidate files, and also
supports manual dispatch. Before any future operational decision, independently verify
source and dependency provenance, exact image and executable identities,
privilege-drop and entrypoint compatibility, persistent-data restore/rollback,
and current full-image scans and symbol-aware findings. The separate libxml2
and other image vulnerabilities remain open; no risk acceptance follows.

The separate opt-in `PostgreSQL Gosu 1.20 candidate` workflow selects a fixed
profile of the same builder against the pinned Alpine AMD64 PostgreSQL image.
It uses unmodified upstream [Gosu 1.20 source](https://github.com/tianon/gosu/releases/tag/1.20)
at commit `29b939aa5bd4dc588af765b8248f6e39150d2565` and observed codeload
archive SHA-256 `2b70da8a5480fafedc16bb1fc91b572f064d03df736cbd91a616af2f1b66ab51`.
That digest is our pin, not an upstream signed checksum or proof that our build
matches the separately signed upstream release executable. This source archive
does not include a vendor directory; the experiment downloads the exact
`go.mod`/`go.sum` dependencies and vendors them before two offline builds.
Gosu and Moby user use Apache-2.0; x/sys uses BSD-3-Clause. The compiled metadata
must identify Moby user v0.4.1 and x/sys v0.1.0. The [Moby advisory](https://github.com/moby/sys/security/advisories/GHSA-mjcv-p78q-w5fw)
identifies v0.4.1 as fixing CVE-2026-61801. A bounded synthetic group file
compares the stock Gosu 1.19's behavior with the candidate's oversized-line
rejection; normal identity and denial controls remain separate. This does not
prove all privilege and compatibility cases.

The unmodified source imports `golang.org/x/sys/unix` for Linux. Its pinned
x/sys v0.1.0 is within the version range for [CVE-2026-39824](https://pkg.go.dev/vuln/GO-2026-5024),
whose affected package and symbol are Windows-specific. This observation is not
a vulnerability scan, an automatic suppression, or a risk acceptance. A future
paired full-image scan must retain all raw rows, including any x/sys finding,
and independently evaluate exact binary reachability and current advisories.
The optional profile creates no deployment or registry publication.

## Alpine plus rebuilt Gosu comparison

The path-scoped Alpine Gosu workflow selects the fixed Gosu 1.20 rebuild profile
and checks its complete evidence and exact executable digest. The builder's
disposable derivative of the pinned PostgreSQL Alpine platform replaces only
`/usr/local/bin/gosu`. The same image ID is used for the builder's identity and
oversized-group checks, the full-image scan and the application restore. An offline archive check verifies the
stock root filesystem prefix, unchanged runtime configuration and the single
root-owned executable layer; the image is neither published nor deployed.

On the disposable runner, the lab checks UID/GID 70, group-set equality with the
stock image, non-root denial, executable identity and an isolated synthetic
PostgreSQL startup/query. It retains full stock and derivative Trivy reports
using one fresh vulnerability database and records all finding deltas, including
remaining and new findings. The comparison requires the expected exact Gosu,
Go standard library, Moby user and x/sys package identities; it retains raw
findings including unknown-severity rows. The x/sys Windows-specific advisory
scope above does not suppress a report row or establish binary reachability.
Comparison results describe scanner reports only;
they do not accept a vulnerability, prove reachability, or establish real data
migration, operational compatibility, production readiness or deployment.

The same hosted job additionally restores an application-schema synthetic dump
from the pinned Debian image into the exact verified Alpine Gosu image ID. Two
new disposable containers use a private internal network without published
ports, random credentials and ownership markers. Small runner-owned loopback
relays forward fixture connections only to the inspected owned container IPv4
addresses; network identity, membership, image identity and absent port
bindings are checked first. The runner accepts only subnets within the three
reviewed RFC 1918 networks in `ops/db-gosu-candidate/restore-private-ipv4.json`;
the verifier pins and strictly parses that file before using it. Both disposable databases explicitly initialize
TCP authentication with SCRAM; correct credentials and an incorrect-password
rejection are checked before seeding. The existing synthetic
migration, order, stock and encrypted-envelope fixture and snapshot/table and
sequence comparison are reused. A preserved, quiescent source is checked again
as a limited rollback control; the candidate is separately changed to prove
content and sequence differences are detected. Duplicate and invalid-archive
controls, original/fresh synthetic key checks and database locale metadata are
recorded. The report also records fixed synthetic ASCII, hyphenated, accented
and Arabic literal ordering and case/equality probes under the default and `C`
collations on each database. It compares each result, validates independent `C`
controls and reports observed differences without declaring the locales equivalent.
The source also records a synthetic original stock settlement for one covers-only
order with the merchant component codes `A-C` and `AB`. The lab requires successful
historical read and idempotent replay on the preserved source. It repeats those
operations on the restored candidate and verifies unchanged stock effects. The
report distinguishes compatible replay from a custody rejection accompanied by
an independently observed ordering change for the two codes. Such a rejection
is evidence against switching the database image.
Database clients still run against owned loopback endpoints; no staging data,
volume, keys or credentials are involved.

The same containers also host a separate pair of freshly created synthetic
databases for a populated schema-66 rehearsal. The source receives exactly the
embedded first 66 migrations and an invented guest order and direct linking
proof. Its held snapshot, table content, migration history and sequence state
are compared after logical restore into the same candidate image, before any
upgrade. Only that copy then runs the embedded SQLx migrator to its current
head, validates all migration versions and checksums, reruns it idempotently,
and checks the legacy proof remains redeemable. A fresh snapshot verifies the
preserved source remains unchanged. A temporary mode-0600 synthetic proof bearer
stays in runner scratch and is excluded from retained evidence. This is one
representative synthetic history, not proof for the actual staging dataset,
collations, roles, extensions, providers or deployed application images.

Separate scan and restore status files distinguish a completed scan from a later
restore failure. Retained checksums cover available evidence even on failure;
the overall workflow succeeds only when both checks succeed. The restore status
artifact reports fixed stage/invariant codes, elapsed time and cleanup outcome;
it does not include database credentials, exception text or process output.

The probe covers one original settlement. An isolated clone must still validate
the actual source and candidate collation metadata, restored application indexes,
additional historical custody such as late allocations, and concurrent lock
ordering before any migration decision.

This does not prove physical-volume portability, default-locale equivalence,
all schema definitions, roles and grants, application image startup, a router
switch, business-write reconciliation or an operational rollback. The original
source has no other writers at the checks; sequence values are not covered by
the exported MVCC snapshot. Migration and operational acceptance remain separate.

## PostgreSQL Alpine comparison lab

The path-scoped `PostgreSQL Alpine comparison lab` workflow compares the pinned
PostgreSQL 18.7 Debian AMD64 image with the independently pinned official 18.7
Alpine 3.24 AMD64 image. It checks registry manifest and config bytes, then uses
separate disposable containers with network disabled and temporary data volumes.
A synthetic custom-format compressed dump from Debian is restored into a new
Alpine database. It checks exact non-ASCII rows, an independently specified
`C`-collated unique-index order, duplicate rejection, sequence state and an
invalid-archive negative control. The two systems' PostgreSQL UID/GID, locale
provider, encoding and collation metadata are recorded rather than presumed equal.

The lab also captures complete unfiltered Trivy reports for both immutable images
using the same fresh vulnerability database, with a positive finding control for
CVE-2026-6653. Its comparison reports both presence and absence of that finding;
it does not waive any finding, establish reachability, or declare either image
safe. A synthetic logical restore and bytewise `C` ordering cannot establish
compatibility of an existing physical volume, application extensions or default
locale semantics across libc implementations. Actual restore, ownership, indexes,
collations, extension compatibility, current security disposition and deployment
remain separate gates. The lab never connects to or alters staging or production.

## PostgreSQL source feature smoke

The path-scoped `PostgreSQL source feature smoke` workflow is an optional,
synthetic experiment on a disposable hosted AMD64 runner. It builds upstream
PostgreSQL 18.7 from the project's HTTPS source archive after checking the
pinned SHA-256 `441dfae4280cae3e0425e5f876f80559fb0731c140f1aae22b3e5a54d0411bfd`
before extraction. The archive and upstream checksum share an HTTPS origin;
this pin is not an independent release signature. The exact pinned stock PGDG
image supplies both build and runtime bases. The locked direct build dependencies come from a timestamped Debian Trixie
snapshot and its dated security snapshot; APT verifies both with the pinned
base's Debian archive keyring, while
expiry checking is disabled for the historical snapshot. The complete installed
package inventory and trust-input hashes are recorded. Transitive packages are
not individually locked, so this is not a reproducible build. The lock's
source and index hashes are documented in
`ops/db-source-feature-smoke/PACKAGE-PROVENANCE.md`.

The new binary enables ICU, OpenSSL and zlib, and disables GSSAPI, LDAP and
libxml/libxslt. The workflow captures each image's PostgreSQL configure flags,
architecture, available locales, libc version, full installed-package list and
resolved libraries for the server and client library. It checks selected missing
libraries in the candidate's resolved linkage, with stock positive controls;
this does not establish their absence from the full image. The runtime retains
the stock image and its packages. Synthetic non-root, network-isolated clusters
check SCRAM success and the expected wrong-password diagnosis, stock-to-candidate compressed
custom archive restore, SQL/XML availability and its specific unsupported-feature
SQLSTATE, and invalid-archive
rejection. Retained evidence includes source and image identity, build output,
fixture results and checksums. Fixture files are transferred through the owned
container processes with size limits and matching SHA-256 checks; status records
the failed fixture stage without database output. Passwords are fixed synthetic
values and are
not included in the evidence.

This is a binary and feature smoke, not a compatible replacement image or a
privilege, vulnerability, collation, TLS handshake, staging or release acceptance.
It does not compare actual database roles, extensions, schema or data, inspect
the entire executable tree, scan images, validate the official entrypoint or
exercise an operational rollback. Actual-clone collation and custody checks,
security disposition and a reviewed operating model remain separate work. The
workflow publishes no image and makes no deployment or staging change.
