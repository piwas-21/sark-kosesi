# Implementation status

**Current stage: P2 catalogue/BOM started; production disabled.**

The read-only Rust/SQLx catalogue, migrations, typed contracts, API/worker readiness
and courtyard Next.js shells are implemented. Explicit clock/identifier ports
support deterministic diagnostics and generated request correlation. Real
source/coverage/audit, PostgreSQL, component/browser/accessibility, approved visual,
performance and discovery gates are required from the first application PR.
Current-revision CI, independent review and the merge gate remain mandatory.

Product families, separable variants, shared cover/foam components and sealed,
immutable BOM revisions are implemented behind the existing read-only preview.
The database rejects fill mismatches and serializes sealing against line edits.
Existing P1 fixtures receive no invented BOM; prepacked stock remains unavailable.
Physical component stock now has append-only movements, receiving/counting,
quarantine inspection/release/disposal, reasons and actor records. The trusted
Rust primitive serializes component changes and rejects conflicting effect-key
retries; no warehouse HTTP operation is exposed.
Atomic holds now reserve a sealed BOM snapshot in physical-component order.
Release, consumption and expiry are distinct, owned, idempotent transitions;
covers-only never consumes foam. Clock inputs are explicit, with a maximum
30-minute window. The worker now has an explicit non-production expiry command:
one pass claims at most 1–100 due holds with `SKIP LOCKED`. Failed effects roll
back, retain redacted retry state and retry after 60 seconds. The default probe
remains read-only. No production scheduler or payment recovery is enabled.

Next work: publication validation, guarded catalogue mutations and projection
invalidation under the [implementation plan](docs/IMPLEMENTATION.md). P3 owns
late-payment reallocation. Supervision of expiry remains a deployment gate.
P0 inputs remain pending. All seven languages are draft previews,
fixtures are explicitly not for sale, and admin operations remain unavailable.
Server-side capability guards now protect catalogue/BOM inspection and warehouse
commands using persisted staff sessions, recent MFA attestations and grants.
Warehouse changes require explicit confirmation. Revocation locks span the stock
commit; audit actors come from the session.
Installation seeds no staff authority. Login/MFA verification, session issuance
and HTTP admin writes remain unimplemented; publication mutations must adopt the
same guard.
Do not restart stack comparisons or redesign the accepted courtyard direction.

Remaining delivery sequence: P0 business inputs in parallel → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.

The repository is public and the protection audit passes. Current-revision CI
remains mandatory, including successful evidence uploads, browser acceptance and
populated Sonar/CodeQL results; do not merge based on local checks alone.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.
