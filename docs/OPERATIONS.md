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
Production remains disabled. The empty migrated preview is useful for integration
wiring; no real merchant facts, tax/carrier readiness, staff authority, order flow
or provider acceptance is established by its deployment. Record accepted source,
image identity, TLS/auth/noindex/RTL smoke and rollback separately from CI.
