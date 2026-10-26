# Product scope and required inputs

Seller: Piwas, Netherlands. Stock: Netherlands. Launch intent: all 27 EU countries. The initial catalogue is narrow and model-led. Brand/domain remain undecided; “Dicle” is a working design name. The approved courtyard design owns the visual direction; sample imagery, dimensions and prices are not catalogue facts.

## Customer experience

- Discover a small curated collection with authentic Diyarbakır context, permissioned photography, responsive art direction, custom consistent icons and purposeful optional motion. Avoid generic catalogue clutter. Product facts and purchase controls remain clear.
- Configure size/textile/fill; show exact included components and explicit covers-only exclusion of foam. Show measured footprint, tolerances, fibre/care, foam firmness/density, stock, dispatch estimate, postcode delivery and bulky-return costs before payment.
- Guest checkout and optional accounts; verified registration, password recovery, orders/shipments/returns/invoices, favourites and variant-specific restock alerts. Link guest orders only after proof of email/order ownership. No mandatory signup to purchase.
- Transactional emails for verification, order/payment/dispatch/return/refund/security and stock availability. Marketing consent is separate; alerts can be cancelled. Accessible localized templates and real bounce/retry handling.
- Product reviews eligible after verified purchase; separate product, support and app feedback. Explain moderation, publish negative reviews under the same rules, allow replies and appeals, avoid review gating. Do not mix support scores into product ratings. Ask at appropriate moments, rate-limit requests and allow dismissal.
- Plan a room with measured units, rotation, tolerances and overlap/door clearance guidance; explain fit limitations. Keyboard/form equivalents for pointer interactions. Photo placement later, calibrated by a user-supplied measurement; it cannot promise automatic accurate dimensions.
- Dutch, German, French, English, Arabic and Turkish plus Kurmanji. Explicit locale URL → saved choice → supported device language → Dutch. Stable localized URLs; Kurmanji internal `kmr`, public `/ku/`, HTML `ku-Latn`, search `ku`. Arabic RTL, native review and no falsely complete fallback translations.
- System/light/dark, system by default; persist an explicit choice without a first-paint flash. Storage denial cannot break the UI. Reduced motion, keyboard use, visible focus, zoom and screen-reader announcements are part of the design.

## Admin operations

The opening screen prioritizes orders to pack, payment/delivery exceptions, low stock, new messages and failed jobs. Every action is authorized by Rust, not merely hidden in the UI.

| Role/capability | Operations and safeguards                                                                                                                         |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Catalogue       | Variants/BOM, dimensions, media rights, translations, prices, safety/traceability and guides; preview/publish with validation and version history |
| Warehouse       | Receive/count/quarantine/reserve/adjust, pick-pack, split shipments, tracking, return inspection; movement reasons and batch traceability         |
| Support         | Inbox linked to orders/customers, assignment, internal/customer notes, templates, safe attachments; ownership and PII minimization                |
| Finance         | Reconcile payments, bounded full/partial refunds, credit documents and exports; reason, permissions and immutable audit trail                     |
| Customers       | Accounts, consent, favourites/alerts, verified guest links, privacy export/deletion; no password viewing or silent impersonation                  |
| Promotions      | Fixed/percentage codes, eligibility, dates, budget and redemption limits; server totals, price history and allocation                             |
| Reviews         | Eligibility, moderation reason, appeal, reply and photo consent; separate product/support/app metrics                                             |
| Operations      | Release SHA, redacted logs, health, failed jobs and bounded retry; no shell, arbitrary SQL, raw secrets or unrestricted customer-data export      |

Owner/admin grants are least-privilege; admin MFA is mandatory. Sensitive actions require recent authentication and explicit confirmation. Simulated/admin fixture data stays outside production.

## Evidence to collect before launch

| Owner                | Deliverable                                                                                                                                             | Blocks                            |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| Piwas                | Brand, domain, seller/invoice identity and contact/return address                                                                                       | Public site and provider approval |
| Supplier + warehouse | Real SKU/BOM, separable/prepacked policy, measured dimensions/tolerances, foam specs, packaging weight/volume/count, receiving and quarantine procedure | Stock, fit and shipping           |
| Supplier/importer    | Material/care, traceability/batches, manufacturer/importer information, relevant safety and label evidence                                              | Product publication               |
| Piwas                | Actual photographs and usage rights; native reviewed translations                                                                                       | Public merchandising/locales      |
| Piwas + accountant   | Landed costs, margin and return reserve; destination VAT/OSS, invoices and credit/refund treatment                                                      | Prices and checkout               |
| Piwas + Mollie       | Approved dedicated retail website profile under eligible Piwas organization, separate test/live keys, methods and payout evidence                       | Live payments                     |
| Carrier + Piwas      | One readiness row per EU country, postcode/territory exceptions, bulky service/access, costs, estimates, returns and packaging obligations              | Country activation                |
| Piwas + advisers     | Product/importer, consumer, textile/packaging, privacy/cookies and accessibility obligations for actual goods/markets                                   | Legal launch acceptance           |
| Operations           | Sender domain SPF/DKIM/DMARC, templates, delivery/bounce checks, support staffing and promises                                                          | Customer operations               |
| Engineering          | Separate hosting, restore/rollback, alert ownership and capacity evidence                                                                               | Production                        |

Create the market matrix from evidence for AT, BE, BG, HR, CY, CZ, DK, EE, FI, FR, DE, GR, HU, IE, IT, LV, LT, LU, MT, NL, PL, PT, RO, SK, SI, ES, SE. Every row is pending/blocked/ready; missing prices or rules fail closed. Special territories/postcodes require explicit handling. Language support does not imply delivery readiness. Unready destinations cannot take payment; resolving them remains a blocker against EU-wide launch intent.

Obtain qualified current review of applicable legal/tax requirements. Ordinary size/foam choices are not automatically a withdrawal-rights exception. Do not invent certifications, testimonials, origin claims or shipping promises to fill missing inputs.
