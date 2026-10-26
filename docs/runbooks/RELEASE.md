# Release, verification and recovery

No deployment is currently configured. `main` is the production source branch, not evidence that a shop is online. The foundation repository is safe to begin implementation; it is not launch-ready.

## PR and environment flow

Feature → independently reviewed PR → develop CI → protected merge → isolated preview/staging after infrastructure exists. Promote through a reviewed develop → main release PR, using a merge commit. Build an immutable image from the accepted SHA; record image digest, environment configuration version and schema version. Avoid build-time public configuration reuse between preview/production images.

Before any rollout: audit branch protection/required contexts and security scan availability, verify matching source/image SHA, migration plan, backup age and restore proof, resource capacity and rollback compatibility. Deploy jobs must use protected environment approval and short-lived/scoped credentials where supported. No live secrets in PR workflows. Production activation is separately authorised.

## Evidence package

Generate an evidence package after the release candidate is immutable: `.local/releases/<sha>/evidence.json` locally or an equivalent access-controlled CI artifact. It includes `head`, and each `config/delivery.json` production gate with `status: passed` and an evidence path below the repository root/artifact directory. Do not commit a file claiming to contain its own final commit SHA. Review the evidence content and source/environment scope, not only the JSON fields.

Run `npm run release:check`. It requires application stage, real app files, production activation, complete current-head evidence and a successful live GitHub protection audit. The current foundation fails this command intentionally. The checker is one gate, not permission to make a live financial transaction.

## Required operational acceptance

- Actual legal/supplier/product/media facts, native-reviewed languages, and all advertised market/return/tax/shipping policies.
- Provider test-mode flows and separately authorised controlled live payment/refund with payout reconciliation. No use of Sofra keys or subscription data.
- Serving revision readback, protected admin, real server authorization, correct checkout totals/state and redacted failures.
- Public discovery contract on the exact canonical host, private/preview noindex, no stale price/availability feed, domain ownership and engine inspection status recorded separately.
- Representative mobile performance, accessibility and reduced-motion review; browser lab thresholds do not establish field p75 until traffic exists.
- Real receiving/pick/pack/parcel/tracking/return inspection procedure, customer support and email delivery acceptance.
- Backup/restore and rollback rehearsal proving the stated recovery targets; alerts exercise with a known injected failure.

## Rollback and data

Use expand/contract migrations, backwards-compatible outbox/event schemas and old/new worker coexistence. Roll back application images when possible without destructive DB rollback. A paid order or issued refund cannot be rolled back by restoring an old database snapshot. After an actual restore, reconcile provider payments/refunds, order/stock allocations and outbound events before reopening sales.

Keep a feature kill switch for checkout that leaves paid-order access/reconciliation/support working. A provider outage should produce a recoverable pending/error state; never mint a second attempt merely to hide uncertainty. A carrier/email outage must not undo paid orders.

## Visibility changes

Run `python3 scripts/audit_github.py` before and after changing repository visibility or the organisation plan. Public setup currently enables enforced protection and native security features. Returning to private on GitHub Free is a known protection gap: do not infer safety from a stored ruleset file or previously green checks. Keep production disabled until equivalent server enforcement is available and reverified.

After merging a release, open a `main → develop` synchronization PR. Review its full diff against develop, record the current main head with `--base origin/develop`, and merge through the same gate. The gate uses a merge commit and retains both protected branches. Confirm main is an ancestor of develop before beginning another release; do not squash away this ancestry.
