# Current implementation roadmap

**State:** product research and direction-02 design are available; repository foundation and implementation handoff are configured. Commerce implementation has not started. Brand, real catalogue, provider onboarding and carrier/market contracts remain open. Production is disabled in `config/delivery.json`.

**Next slice: P0.1 commerce proof.** Read the [handoff](docs/plans/IMPLEMENTATION-HANDOFF.md) and [ADR-001](docs/architecture/ADR-001-COMMERCE-FOUNDATION.md). Build isolated executable evidence for the candidate engine before production scaffolding.

| Phase | Deliverable | Exit condition | State |
| --- | --- | --- | --- |
| Foundation | Standalone repo, migrated materials, rules, CI, branch protection and handoff | Current head verified by all configured checks and merge policy | Configured; current develop checks are the verification source |
| P0.1 | Medusa/Next.js decision proof | Kits/last-stock concurrency, Mollie lifecycle, late pay, country quote, locale and permission proof; accepted ADR | Next |
| P0.2 | Product and fulfilment contract | Real SKU/component model, sample measurements, package/shipping policy, supplier and 27-market checklist | Owner/supplier inputs pending; parallel with P0.1 |
| P1 | App foundation, routes/themes/auth boundaries, CI | Actual application type/lint/build/test/security/performance/discovery jobs required; isolated preview | Pending |
| P2 | Catalogue, inventory, quote and cart | Immutable quotes, component holds, market rules, realistic seeded fixtures, admin receiving/adjustments | Pending |
| P3 | Checkout, Mollie, orders, fulfilment, returns | Provider sandbox evidence, recovery/reconciliation, idempotency, stock races and refund allocation tests | Pending |
| P4 | Accounts, communications, support, feedback, operations | Ownership/consent/outbox tests; role matrix; diagnostics and worker failure controls | Pending |
| P5 | Discovery, measured fit/photo placement, launch rehearsal | Performance/security/accessibility/discovery evidence; native language review; restore and carrier proof | Pending |
| P6 | Optional 3D/camera AR | Measured assets and device validation; clear fit limitations | Deferred |

Do not wait for branding to begin isolated technical proof. Do not activate public sales without the applicable real product, legal, payment and destination evidence. Preserve EU-wide intent; a country cannot silently appear orderable before its shipping/tax/returns rules are validated.

## Decisions still open

- Final brand/domain and exact first models/components. No trademark clearance implied by Dicle.
- P0 engine decision, maintained Mollie adapter versus reviewed custom implementation, admin permission/localisation approach.
- Carrier/parcel and bulky-return costs, customs/importer/product safety/packaging obligations, VAT treatment and accounting review.
- Native-reviewed catalogue and transactional language coverage, real product photography and measured assets.
- Production hosting, resource isolation, mail sender identity, backup storage and incident ownership.
- Visibility after setup: keep enforced protection available before production; do not count local hooks as equivalent.

See [launch inputs](docs/product/LAUNCH-INPUTS.md) for owners and evidence, and [CI gates](docs/engineering/CI-GATES.md) for implemented versus pending checks.
