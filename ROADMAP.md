# Implementation status

**Current stage: P1 preview application in development; production disabled.**

The read-only Rust/SQLx catalogue, migrations, typed contracts and courtyard
Next.js shells are implemented on an owned feature branch. The first application
PR activates real source/coverage/audit, PostgreSQL, component/browser/accessibility,
visual, performance and discovery gates together. The initial visual baselines
have user approval. Current-revision application CI and independent review must
still pass before merging; local test results do not establish this exit evidence.

Next work: finish [P1 acceptance](docs/IMPLEMENTATION.md), then P2 catalogue/BOM
and stock. P0 inputs remain pending. All seven languages are draft previews,
fixtures are explicitly not for sale, and admin operations remain unavailable.
Do not restart stack comparisons or redesign the accepted courtyard direction.

Remaining delivery sequence: P0 business inputs → P1 application skeleton and real gates → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.
