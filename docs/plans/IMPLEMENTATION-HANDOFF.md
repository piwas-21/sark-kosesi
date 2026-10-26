# Full implementation handoff

## Starting state

Use this repository as the working directory. Read AGENTS, ROADMAP, this file, ADR-001 and the commerce plan. The direction-02 design is the preferred visual reference. The code under `design/prototype` is a local demonstrator: do not mistake its fake data, browser-only basket or 2D fit calculation for implemented commerce.

The repository owns its files and dependencies independently of RUMI/Sofra. Foundation CI runs documentation/asset integrity, tooling tests, JavaScript parsing, prototype browser scenarios, CodeQL, dependency audit and secret scanning. GitHub rules require these results for PRs. Production gates remain pending in `config/delivery.json`; no deploy workflow is installed.

## First session: execute P0.1

1. Check status and latest `origin/develop`. Create an owned branch/worktree. Run `npm ci --ignore-scripts`, install hooks, then `npm run check`. Inspect recent CI; do not assume a remembered result still applies.
2. Read the existing plan’s commerce, payment, architecture and market sections. Check current stable Next.js/React/Medusa/Node compatibility and advisories using vendor sources. The foundation pins Node 24.22.1; evaluate framework versions together and commit the exact resolved set.
3. Build an isolated `spikes/commerce-proof/` with disposable PostgreSQL and synthetic catalogue. Use no production database, live payment key or customer data. Give services unique ports, names and volumes. Explicitly allowlist disposable database URLs before destructive setup.
4. Prove each ADR-001 acceptance scenario. Use a fake provider at the adapter boundary for fault ordering, then separate Mollie test-mode evidence when keys are available. A fake provider cannot establish Mollie compatibility. Missing credentials should not stop the local concurrency/recovery proof.
5. Record actual commands, assertions, engine versions and remaining provider evidence in the ADR’s evidence table. Choose the engine from the proof. If Medusa fails, document the exact missing invariant and compare a custom modular monolith; do not run two stock/order systems.
6. Translate the successful spike into a supported production shape in P1. Remove spike-only infrastructure when superseded. Do not copy throwaway bypasses or fake providers into production defaults.
7. Make an independently reviewed PR to develop, verify CI for its head, merge via the gate, and update ROADMAP with the next executable slice.

## Ordered implementation slices

| Slice | Scope and dependency | Required evidence before merge/phase exit |
| --- | --- | --- |
| P0.1 | Commerce engine + provider contracts | ADR proof matrix; transactional stock race; unknown provider outcome; role/localisation feasibility |
| P0.2 | Catalogue/component and market contract | Real input schema, supplier checklist, 27-country matrix with unready countries explicit; can proceed alongside P0.1 |
| P1.1 | npm workspace, apps, environment validation, isolated DB/worker/storage | Production build/type/lint/unit/integration/security jobs wired in same PR; migrations fresh + upgrade; no fake green commands |
| P1.2 | Design tokens, semantic primitives, locale/theme routing | SSR content; all seven core UI locales; Arabic/long-copy layouts; storage-denied/device/default behavior; accessibility baseline |
| P1.3 | Authentication and admin boundaries | Guest path, account verification/recovery, session revocation, MFA for admin, endpoint role/ownership negative tests |
| P2.1 | Catalogue/media/publication | Versioned facts, components, translations and images; publish eligibility and honest discovery coverage |
| P2.2 | Inventory/quote/cart | Real transaction reservations, concurrency and expiry; cents/VAT/discount/shipping snapshots; destination quote rules |
| P3.1 | Mollie checkout/reconciliation | Stable attempts, fetch/verify, race/replay/timeout recovery, pending return UX; sandbox integration |
| P3.2 | Orders/packing/tracking/returns/refunds | Per-parcel tracking, partial fulfillment, quarantined returns, bounded refunds and credit allocation; audited operations |
| P4.1 | Accounts, favourites/restock/email | Anonymous continuation, cross-account denial, verified consent, durable outbox and unsubscribe/claim policy |
| P4.2 | Support, product reviews, service/site feedback, discounts | Verified eligibility, moderation/audit, no review gating, separate score types, discount history/eligibility and abuse tests |
| P4.3 | Operations and safe diagnostics | Roles, redacted logs, controlled job retries, metrics/alerts, backup and retention jobs |
| P5.1 | Search/merchant/content/measurement | Emitted HTML + canonical/hreflang/JSON-LD/feed consistency, expected sitemap coverage, private noindex, publisher invalidation |
| P5.2 | Measurement/local room placement | Fit math and obstruction limits clear; local-only images by default; accessible fallback; no measurement accuracy claims from AI |
| P5.3 | Launch rehearsal | Security, performance, accessibility, translations, real media, provider/carrier, restore/rollback and legal/product evidence |
| P6 | Optional measured 3D/AR | Separate budget, actual assets, physical scale/device tests; no impact on first-paint bundle |

Feature PRs should be reviewable vertical slices, not one application-sized PR. Wire a test before or with the behavior it establishes. Update the evidence matrix as real gates become implemented. Do not label a framework starter as P1 complete.

## Decisions delegated to implementation

Choose supported dependency patches, component boundaries, indexing strategy, job implementation and equivalent test tooling within the documented budgets. Use CSS Modules/tokens for the custom storefront. Start with the smallest adequate service topology. Record substantial architecture changes in ADRs and their impact on estimates. Do not ask the owner to approve routine fixes or every library patch.

## Inputs that remain owner/external work

The brand/domain, supplier facts/media rights, product specifications, landed costs, carrier/returns contracts, tax/importer/market obligations, email identity, Mollie account/profile approval, and native-language approval are not established. See the launch-input matrix. Continue independent engineering with synthetic fixtures; keep affected sales/publication capabilities disabled until real inputs pass validation.

No live payment, subscription, carrier purchase, provider contract, customer email or production deployment is authorised by this handoff itself. Technical preparation and sandbox work may continue. A separate explicit instruction can authorise the relevant external action later.

## Ready-to-paste next-session prompt

> Work in this repository. Read AGENTS.md, ROADMAP.md, docs/plans/IMPLEMENTATION-HANDOFF.md and ADR-001. Begin P0.1 and progress through the planned implementation slices, preserving the direction-02 design and the Piwas/NL-stock/EU-wide requirements. First prove shared component inventory, Mollie attempt/reconciliation semantics, late-paid recovery, country quotes, translations and admin permissions in isolated test infrastructure; then record the engine decision. Keep checks real, add application CI before adding production apps, and use reviewed PRs to develop. Do not use live Sofra resources or infer production readiness from prototype CI. Update the roadmap and explain any genuine external blocker while continuing independent work.
