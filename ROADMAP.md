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
30-minute window. No expiry scheduler or payment recovery is implemented yet.

Next work: publication validation, catalogue/warehouse permissions and projection
invalidation under the [implementation plan](docs/IMPLEMENTATION.md). Add a bounded
expiry worker before operational stock use; P3 owns late-payment reallocation.
P0 inputs remain pending. All seven languages are draft previews,
fixtures are explicitly not for sale, and admin operations remain unavailable.
Do not restart stack comparisons or redesign the accepted courtyard direction.

Remaining delivery sequence: P0 business inputs in parallel → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.

Delivery is also blocked: the repository currently reports private, GitHub denies
branch-protection/ruleset access and CodeQL uploads, and artifact storage quota
prevents CI evidence uploads. Keep every gate active. An owner decision on public
visibility or an eligible protected private setup is pending; do not merge based
on local checks alone.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.
