# Test strategy and acceptance matrix

Tests establish behavior, not just execution. Use independently specified expected amounts/states, not the same helper as the code under test. Mutation/negative controls are useful for critical invariants and parsers that otherwise return empty/green. Do not chase a percentage by mirroring trivial implementation details.

## Layers and fixtures

- **Unit:** cents/tax/discount/refund allocation, reservation/state predicates, locale negotiation and fit math. Table-driven examples and property tests for conservation, boundaries and replay. Target >=90% branch coverage for payment/inventory/money modules and >=80% for the app overall once substantive code exists; explain excluded generated/adaptor code.
- **Integration:** real disposable PostgreSQL, real migrations and durable jobs; provider adapters controlled at the network boundary. Separate DB per worker/suite. Explicit local test DB allowlist before destructive reset. No live/tenant/development DB.
- **Contract:** schema validation for storefront/admin/provider adapters, event versioning, compatibility between rolling API/worker versions, catalogue/feed read models.
- **E2E:** built application and realistic synthetic customer/operator fixtures; stable semantic selectors; deterministic clocks/providers. Chromium/WebKit/Firefox core purchase flows; Chromium wider matrix on PR; full browser/locale matrix nightly and before release.
- **Security/accessibility/discovery/performance:** independent suites with explicit fixtures and controls, described in their specifications. A mock payment is never reported as a provider-sandbox pass.
- **Manual:** real device/assistive technology, native language review, supplier measurements, parcel/access checks, authorised provider payment/refund, payout and restore evidence.

A flaky test is a defect. Diagnose it; do not retry until green or quarantine a release-critical scenario. A narrowly quarantined noncritical test requires an issue, owner and expiry. Mandatory suites may not be empty or skipped.

## Release-critical scenarios

| ID | Scenario | Oracle / expected outcome | Layer |
| --- | --- | --- | --- |
| INV-01 | 50 buyers, one shared cover kit | Exactly one allocation; no negative stock, duplicate holds or paid order silently lost | Integration |
| INV-02 | 5 cover kits / 3 foam kits, mixed options | Combined consumption stays within each component; unavailable foam does not block covers-only | Unit + integration |
| INV-03 | Expiry racing paid webhook | One consistent terminal allocation/refund exception; audit/outbox unique | Integration |
| PAY-01 | Provider creates payment then timeout | Same stable attempt recovered; no replacement charge | Adapter + integration + sandbox |
| PAY-02 | Duplicate/out-of-order/forged callbacks | One transition; wrong profile/mode/currency/amount denied; return URL cannot mark paid | Integration + E2E |
| PAY-03 | Browser closes; webhook missing | Reconciler converges; order lookup recovers; confirmation emitted once | Integration + E2E |
| PAY-04 | Partial refund with quantity discount + VAT + shipping | Exact independent cents fixture; cumulative refund <= captured; no automatic restock | Unit + sandbox |
| ORD-01 | Guest buys, later registers/claims | Identity verified before attachment; email match alone never grants history | Integration + E2E |
| AUTH-01 | Customer A reads/changes B’s order/return | 403/404 as specified; allowed own-resource control succeeds | Integration + E2E |
| ADM-01 | Support attempts refund/export/access change | Server denies; owner allowed with step-up and audit | Integration + E2E |
| FUL-01 | Multiple parcels and partial shipment | Per-item/parcel tracking; delivered state only for delivered allocation | Integration + manual carrier |
| RET-01 | Return requested, received, quarantined, inspected | Refund eligibility/state separate from stock return; reason/actor retained | Integration + E2E |
| DIS-01 | Expired/private coupon, concurrent redemption | Eligibility/limits/stacking enforced on server; no invented prior price | Unit + integration |
| MSG-01 | Email accepted then worker crashes | Retry converges using stable business/provider key; no lost/duplicate confirmation | Integration |
| ALT-01 | Restock alert dedup/unsubscribe/repeated stock changes | One eligible notification per event policy; no marketing consent implied | Integration + E2E |
| REV-01 | Purchased/unpurchased user and moderation | Eligibility visible; no support/site scores mixed with product stars; negative review not suppressed for sentiment | Integration + E2E |
| LOC-01 | Explicit URL, saved preference, device, fallback | Correct precedence; routes/variant/required queries survive; no cross-locale cache contamination | Unit + E2E |
| UI-01 | 320/390/768/1280/1440px, RTL, 200/400% zoom | No page overflow/hidden control; usable keyboard/focus/modal; reduced motion | E2E + manual |
| FIT-01 | Rotated/unrotated exact-fit, no-fit, invalid units | Dimension result matches fixture; no AR/obstruction guarantee | Unit + E2E |
| SEO-01 | Incomplete translations, variants, private routes | Expected public pages present; incomplete/private absent; reciprocal accurate alternates | Served-output |
| MIG-01 | Previous-release DB and in-flight jobs | Upgrade preserves snapshots; old/new workers compatible; no schema drift | Integration |
| OPS-01 | DB/object restore and application rollback | Measured RPO/RTO; order/payment states reconcile without reverting financial truth | Staging rehearsal |

## Current foundation evidence

The prototype suite exercises observable covers-only price/basket behavior, fit/no-fit, seven-locale viewport checks, device theme/language, RTL, reduced motion and product accessibility. It deliberately does not claim inventory, auth, payments or server SEO behavior. Tooling tests validate the merge/release gates with missing, stale and failing fixtures.

## Artifacts and release record

Keep test reports/trace/screenshots only on failure where practical, with short CI retention and synthetic data. Do not publish customer screenshots or provider secrets. For a release candidate, collect evidence under `.local/releases/<head-sha>/` or the corresponding protected CI artifact. Record source SHA, tool/browser versions, fixture version, command, environment, result and reviewer. Evidence is produced after the immutable candidate commit; do not create a self-referential committed “current SHA” file.
