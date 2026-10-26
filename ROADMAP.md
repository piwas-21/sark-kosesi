# Implementation status

**Current stage: foundation; production disabled.** The approved design, selected Rust architecture, implementation plan and repository checks are the starting point. No production commerce API, database, storefront or payment integration exists yet.

P1 coverage-report validation is implemented with positive and negative controls. Actual application coverage and application CI remain pending.

Next work: [P1 in the implementation plan](docs/IMPLEMENTATION.md). Establish the real Rust workspace, database migrations and server-rendered frontend; activate application CI in that same PR. Complete P0 business inputs alongside engineering. Do not restart stack comparisons or redesign the accepted courtyard direction.

Remaining delivery sequence: P0 business inputs → P1 application skeleton and real gates → P2 inventory/catalogue → P3 checkout/payments → P4 customer/admin operations → P5 room planning and content → P6 acceptance → P7 controlled launch. Each phase has explicit exit evidence in the plan.

External blockers: final brand/domain, real product specification/media, market/carrier/tax readiness, Mollie retail website approval, translated legal/product content, email delivery and recovery evidence. EU-wide intent is unchanged; an unready country cannot take payment.

Before any private-repository switch, run the protection audit and follow [operations](docs/OPERATIONS.md). A GitHub Free private repository does not retain the current public-repository enforcement. Do not declare production readiness while that is unresolved.
