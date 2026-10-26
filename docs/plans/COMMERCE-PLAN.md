# Piwas şark köşesi shop — development and launch plan

**Status:** proposal for design and scope discussion; no production implementation approved by this document.
**Prepared:** 23 October 2026.
**Seller:** Piwas, existing Netherlands company (confirmed by owner).
**Fulfilment:** import from Türkiye/Diyarbakır, inventory held in the Netherlands, EU-wide launch (confirmed by owner).
**Brand:** undecided. “Dicle” is a working title for the design preview only; no trademark/domain clearance.
**Design:** [interactive local preview](http://127.0.0.1:48763/studio.html) · [saved design notes](../../design/prototype/DESIGN-NOTES.md). The HTTP preview requires its local server; the saved HTML/assets also work directly.
**Research:** [source-backed comparison and findings](../research/commerce-research.md).
**Existing discovery reference:** [public search discovery runbook](../discovery/SEO-AEO-GEO.md), adapted to retail below.

## 1. Product decision

Build a mobile-first, multilingual retail web app around a deliberately small collection of traditional low seating. Differentiate through atmosphere, clear product contents and confidence about fit. Keep payment and delivery familiar.

Recommended launch shape: one seating family, one or two verified patterns, at most two standard sizes, each offered **with foam** and **covers only**. This is a proposed ceiling of 4–8 sellable configurations, not an assertion about supplier inventory. Do not publish arbitrary sizes just because the configurator can render them. Choose the actual catalogue after samples, package measurements and margins are known.

All seven requested interface languages remain in launch scope: Dutch, German, French, English, Arabic, Turkish and Kurmanji. All 27 EU countries remain the commercial launch target. Non-EU neighbours (UK, Switzerland, Norway, Türkiye) are a separate future workstream because they add customs, tax and delivery requirements.

### User requirements → delivery commitment

| Requirement | Proposed implementation | Phase |
|---|---|---|
| Online order/payment | Guest-first checkout, Mollie hosted one-off payment, verified settlement, full/partial refunds | P2–P3 |
| Existing Mollie account | Separate website profile and keys under the same Piwas organisation, pending legal-identity and profile approval checks | P0, P3 |
| Foam / no foam | Named configurations, explicit contents, component stock, separate package/shipping models | P1–P3 |
| Inventory/orders/messages/users/discounts | Role-controlled admin with audited changes and operational queues | P2–P4 |
| Debugging/logs | Read-only redacted diagnostics, job state and controlled retries; observability links for the owner | P1, P4 |
| Light/dark, device default | System/light/dark preference; first-render correct theme; persistent explicit choice | P1 |
| Seven languages, device default | Locale URLs, negotiated first entry, persisted selection, Arabic RTL, native review | P1–P5 |
| Registration/order history/emails | Optional account, guest order access, secure claiming, localized transactional email | P3–P4 |
| Favourites/back-in-stock | Guest favourites locally; account sync; verified variant-specific stock alerts | P4 |
| Product/system/support feedback | Separate feedback channels, post-delivery verified-purchase reviews, fair moderation | P4 |
| Try in a room / fit | Measured 2D layout and local photo placement at launch; true-scale 3D/AR after actual models | P4; P6 for AR |
| Performance/security | Explicit budgets, hardened auth, ownership checks, inventory/payment failure tests, restore drill | Throughout, P5 |
| SEO/GEO | Server-rendered facts, product schema, multilingual discovery, merchant feeds and measured reporting | P1, P4–P5 |
| Distinctive living design | Direction 02: The living courtyard, tactile imagery, reactive configuration, restrained motion | Design gate, P1 |

Recommended exclusions from the first release: native iOS/Android apps, marketplace/multiple sellers, subscriptions, loyalty points, BNPL, customer-generated AI decoration, arbitrary made-to-measure orders, multi-warehouse optimisation and a chatbot that guesses product facts. These add cost without resolving the initial buying decision. The web app remains installable if a lightweight PWA is useful; never queue checkout or payment offline.

## 2. Design to review before building

**Direction 02 — The living courtyard:** an immersive basalt courtyard, monumental typography, forest green, pale pistachio and pomegranate textiles. The seating silhouette anchors the design. A textile detail opens from the scene; a sculptural room model introduces the fit planner. This is the current discussion proposal, not an approved production design.

- Colour tokens: forest `#153D31`, pistachio `#E4ECB3`, pomegranate `#933B4A`, mineral green `#E9EDDF`, ink `#183C32`, leaf `#B1C09C`. Contrast-tested semantic pairs replace direct component colours in production.
- Typography: locally hosted Bricolage Grotesque for sculptural display and DM Sans for controls; production needs a paired Arabic family and native-reader review of Arabic, Kurmanji and Turkish. Font licences are saved with the preview.
- Layout: immersive courtyard → seating cutout composition → interactive room invitation → origin story. Mobile recomposes the scene vertically. Product pages use an arched image beside a compact configurator; admin uses restrained lists and a consistent SVG icon family.
- Motion: coordinated opening, optional subtle pointer response, pausable rotating planner seal, textile disclosure, responsive room model, favourite feedback and footprint transitions. Reduced motion disables nonessential animation. No autoplay video, scroll hijacking or endless carousel.
- Product truth: real stock photos, material close-ups and actual dimensions for selling. AI interiors may be labelled inspiration; never substitute for measured product photography or present the attached reference as Piwas’s own product.
- Alternative directions: textile atelier (more material detail), or family gathering (documentary people/homes). See the research note. The current interactive proposal develops the living courtyard.

The local preview covers storefront, product configuration, sample basket, favourites, a 2D fit check and admin. Seven-language core copy is draft; some demonstration explanations/admin text remain English. It has no payment, account, inventory, email, uploads or AR connection. Prices, dimensions and orders are fictional. The reference and generated room are labelled.

**Design acceptance:** review at 390px, 768px, 1280px and 1440px; light/dark; Arabic RTL; a long German/French heading; keyboard-only product selection. Agree on visual direction, tone, homepage priority and product terminology before production components. Brand naming can progress separately, but domain/legal identity must be settled before provider onboarding and public launch.

## 3. Site map and customer journeys

### Public and account surfaces

- Home, focused collection, product/configuration, room planner, origin/supplier story, measuring guide, foam-versus-covers guide, care guide, shipping/returns, help/contact, product reviews, legal/privacy/accessibility pages.
- Basket, guest checkout, provider handoff, payment-pending/result, order tracking, returns and support cases.
- Optional register/sign-in/recovery, profile/address book, orders/invoices, favourites, saved configurations, stock alerts, email preferences, privacy requests.
- Start without a large search/filter interface: a tiny catalogue needs clear choice. Add basic search when the catalogue justifies it; keep accessible collection navigation from day one.

### Main purchase

1. Visitor lands on a language-specific public page, with system theme unless they chose otherwise.
2. Select pattern and standard size; see exact width/depth/height, component count and real images.
3. Select **With foam** or **Covers only**. Update price, stock, contents, weight, packed size and delivery estimate together. A covers-only selection shows a durable “No filling included” statement near the purchase button, in basket, order confirmation and invoice description.
4. Optional planner and sample request; returning to product preserves configuration.
5. Enter destination country/postcode for delivery estimate before checkout. Explain doorstep versus room delivery, appointment, stairs, remote-area handling and assembly where applicable.
6. Buy as guest or sign in; never require registration to see shipping prices or pay.
7. Confirm goods, delivery, discount, destination VAT, grand total and return terms. Show a clear obligation-to-pay button. No optional extras preselected.
8. Reserve stock server-side, create one payment attempt and redirect to Mollie.
9. Return page shows confirmed, pending or failed based on server verification. It must work if the webhook arrives first, later, twice, or the browser never returns.
10. Deliver confirmation and tracking emails. Offer account creation after purchase, with email verification before linking existing orders.

### Recovery and accessibility

Payment failure preserves the basket and offers retry without creating duplicate orders. Out-of-stock changes identify the affected variant. Address errors keep other form values. A shipping quote failure does not silently become free shipping. Support links retain product/order context without exposing private data in URLs. Native fields, meaningful error summaries, focus management, 44px target goal and keyboard-operable diagrams make core flows usable without drag gestures or camera access.

## 4. Product model and warehouse operations

### Product facts required before publication

Stable product/variant SKU; translated title/description; supplier/manufacturer identity; provenance evidence; actual fibre composition; care; measured total and component dimensions; foam type/density/firmness and tolerances; package contents and exclusions; packed dimensions/weight/count; colour/pattern photographs; tax category; HS/CN classification reviewed by importer; safety/traceability information; return condition rules; source document versions.

Avoid unsupported “handmade,” “handwoven,” “orthopaedic,” “non-toxic,” “washable,” “never sags,” certifications and environmental claims. Do not market adult seating as a child-sleep product without the relevant assessment.

### Component inventory

A sellable variant may consume a cover kit and a foam kit. If covers can be sold both empty and filled, those offers cannot own independent copies of the same stock.

Example: 5 available cover kits and 3 foam kits allow at most 3 filled sets, or 5 covers-only sets, subject to the combined reservations. Concurrent orders must share the same component ledger. If sets arrive prepacked and cannot be split, model them as separate physical SKUs instead. The supplier/warehouse decision chooses the model; do not maintain both representations for the same units.

Inventory records: location, component, batch, on-hand, reservation, committed allocation, quarantine, incoming purchase order, movements and reason. Available = on-hand − active holds − allocated − quarantine, with no double subtraction if the selected engine represents allocation as a reservation. The engine spike must settle that semantic detail.

Operations: receive shipment → inspect/count → label batch → release for sale. Pick/pack from paid allocations. Mark tracking per parcel and allow partial fulfilment. Returned goods enter quarantine, then inspected resale/damaged stock; refunds and restocking are separate actions. Inventory adjustment requires reason and actor audit. Never overwrite the count silently.

## 5. Payments and Mollie reuse

**Recommendation: keep the existing Mollie login/organisation if it is registered to the same Piwas company; create a new website profile and new test/live keys.** Mollie explicitly supports this structure. Separate legal registrations need separate organisations, though the login may be shared. [Website profiles](https://help.mollie.com/hc/en-us/articles/212613429-Can-I-add-or-change-a-website-profile), [organisations](https://help.mollie.com/hc/en-gb/articles/360014268520-Can-I-add-another-company-to-my-account)

The current Sofra code is subscription billing: `sofra/lib/mollie.ts:103` creates `sequenceType: first`; `sofra/lib/billing.ts` activates recurring subscriptions. Its HTTP client and fetch-and-verify approach are useful references. Do not reuse its keys, subscription mandates, tenant records, customer database or webhook endpoint for goods orders. Current live-account approval/payout status was not verified; no usable local profile key was available in the checked files.

### Provider onboarding checklist

1. Confirm existing Mollie organisation is Piwas’s registration and verify payout/account standing in the dashboard.
2. Settle shop domain/trade name and publish an accurate non-selling review site with legal details, genuine product descriptions/prices, delivery and returns.
3. Create a website profile with the actual retail category and countries of activity; complete Mollie’s review and activate available methods.
4. Store new profile keys in separate secrets for staging/production; use profile-bound keys, separate webhook and return hosts, and independent settlement reconciliation.
5. Launch candidate methods: cards, iDEAL/Wero as actually offered/approved in the account, Bancontact, and eligible wallets. Fetch method availability for amount, currency and country; do not hardcode the assumption that every method is enabled. Delay BNPL until capture, shipment and return flows are proven.
6. Test in test mode; later perform an explicitly authorised controlled live payment/refund and reconcile the payout. Account support is not evidence of a successful live transaction.

Mollie’s documented hosted locales currently cover five requested languages but not Arabic/Kurmanji. Keep the shop/order/email fully localized; tell the shopper the hosted step uses English (or their explicit supported choice), and restore their original locale afterward. [Payment API](https://docs.mollie.com/reference/create-payment)

### Invariants and state

- Money uses integer minor units and a currency. Server recalculates every line, tax, discount and shipping amount. Freeze an order snapshot; changes require a new quote/payment attempt rather than mutating a pending payable amount.
- One stable idempotency key per attempt; unique provider payment ID and webhook processing record. Do not mark an order paid from the return URL or client event.
- Treat an incoming webhook identifier as untrusted. Re-fetch through the correct profile credentials, match order/attempt, mode, currency and exact amount. Unknown or mismatched records never allocate fulfilment.
- Stock hold before payment. Use engine transactions/locks for shared component holds. A configurable initial hold can be 20 minutes for fast methods, but provider pending states may outlive it. A reconciler must decide whether to extend, expire or release; do not use a blind timer.
- If a late paid event arrives after a release, atomically try to reallocate; if unavailable, create an exception for refund/recovery and notify the customer accurately. Never silently oversell or discard a paid event.
- A timeout after provider creation triggers lookup/retry with the same key, not a fresh charge. Polling/reconciliation recovers missed webhooks. Return and webhook may race without duplicating emails, allocation or invoices.
- Keep payment state separate from fulfilment and returns. `authorized` is not `paid`; a method needing capture must follow its capture contract. Launch can restrict to fully implemented methods.
- Full/partial refunds are idempotent, bounded by captured less refunded amount, and match provider state before the UI says completed. Preserve discounts/tax/shipping allocation for partial refunds and issue the relevant credit document. Chargebacks create a finance task, not an automatic stock increase.
- Durable outbox jobs for confirmation/invoice/stock notification; email failure never rolls back a completed payment. Retrying a worker cannot send duplicate business events.

State projection:

```
Order:     draft → awaiting_payment → confirmed → closed
                       ↘ expired / cancelled        ↘ exception
Payment:   created → pending → paid → partially_refunded → refunded
                       ↘ failed / cancelled / expired
Fulfilment: unallocated → allocated → picking → dispatched → delivered
Return:    requested → authorised → received → inspected → resolved
```

Actual enums follow the selected commerce engine; transitions and event mapping are contract-tested. A return can coexist with partially delivered fulfilment; do not force everything into one order status.

## 6. EU-wide shipping, pricing and importer readiness

The user’s target is **all 27 EU countries at launch**, not a Netherlands-only MVP. Build a `MarketPolicy`/shipping matrix covering AT, BE, BG, HR, CY, CZ, DK, EE, FI, FR, DE, GR, HU, IE, IT, LV, LT, LU, MT, NL, PL, PT, RO, SK, SI, ES, SE. A language is not a shipping country. EUR checkout is proposed for all markets; non-euro residents see a clear EUR charge and possible bank conversion.

For each country and applicable postcode/territory: supported parcel/freight service, cost by package type, remote/island surcharge, realistic dispatch/transit window, delivery level, appointment/phone requirement, return route/cost estimate, relevant VAT and product/packaging/language requirements. Mainland rates do not prove island or special-territory coverage. EU-country membership alone does not determine its entire customs/VAT territory.

Target ordinary supported addresses in all 27; use a precise quote request for exceptional addresses without taking payment until a binding total is available. No “EU-wide free shipping” or fixed delivery promise before carrier agreements and package measurements. A country not operationally ready must be shown honestly; resolving that is a launch blocker against the chosen EU-wide scope, not an implicit scope reduction.

### Commercial checklist

- Supplier samples, purchase terms, defect allowance, batch consistency, replenishment lead time and permissioned photography.
- Transport/import insurance, Incoterms and responsibility; EORI/customs broker; correct tariff classification and A.TR eligibility. Import VAT/duties must be costed rather than assumed away. [Customs](https://business.gov.nl/regulations/declaring-goods-customs/), [A.TR](https://business.gov.nl/international/doing-business-abroad/a.tr-certificate-for-turkiye/)
- Piwas’s accountant confirms destination VAT/Union OSS and any threshold conditions at company level. Prefer a model ready for destination VAT from day one; do not hardcode Dutch 21% for all sales. [VAT](https://europa.eu/youreurope/business/finance-and-tax/vat/cross-border-vat/index_en.htm), [OSS](https://europa.eu/youreurope/business/finance-and-tax/vat/one-stop-shop/index_en.htm)
- Importer/manufacturer role, risk assessment, applicable product rules/tests, traceability and recall process. Verify safety and contact information required in online offers and physical labels. Do not add CE marks without an applicable legal basis. [Importer responsibilities](https://business.gov.nl/products-services-and-innovation/product-safety/product-safety-for-importers/)
- Textile composition and market-specific required languages; seven UI languages are not a complete EU label solution. [Textile labels](https://europa.eu/youreurope/business/product-rules-compliance/textiles-and-footwear/textile-label/index_en.htm)
- Packaging/EPR review by destination, including applicable national furniture/textile streams; collect weights/materials per package. [Packaging](https://business.gov.nl/regulations/packaging/)
- Clear seller/contact identity, Dutch return address, cancellation method, statutory conformity rights, return-cost estimates for bulky items and defective-goods process. Standard pattern/size/foam options do not automatically remove withdrawal rights. [EU distance selling](https://europa.eu/youreurope/business/selling-in-eu/selling-goods-services/ecommerce-distance-selling/index_en.htm), [Dutch webshop rules](https://business.gov.nl/regulations/long-distance-sales-and-purchases/)

### Unit economics before choosing retail prices

Calculate by configuration **and destination**: supplier cost + transport + duty/nonrecoverable taxes + brokerage + receiving/storage + packaging/pick-pack + carrier cost + payment fee + expected damage/returns/support. Treat recoverable VAT correctly with the accountant. Gross contribution = net-of-VAT sales less those costs, discount and return allowance. Test covers-only versus filled sets: volume can dominate foam economics. The preview’s €189/€349 examples are not price recommendations.

## 7. Admin: a working operations desk

Default screen: paid orders to pack, delivery exceptions, low stock, new customer messages, returns awaiting inspection, failed jobs. Avoid a decorative dashboard dominated by meaningless revenue charts.

| Area | Required operations | Guardrails |
|---|---|---|
| Catalogue/content | Edit variants, component contents, translations, prices, photos, dimensions, safety docs, guides; preview then publish | Required-data validation and translation coverage; version history; no price published by accident |
| Inventory | Receive, reserve, count, adjust, quarantine, batch trace, low-stock alerts, supplier lead times | Atomic component movements; adjustment reason; importer recall link |
| Orders | Search/filter, full timeline, payment/delivery status, pick/pack, split shipments, cancellation, returns, refund | State-based actions, idempotency, customer-visible notes distinct from internal notes |
| Messages | Shared inbox linked to order/product, assignment, unread state, internal notes, attachments, templates, status | No sensitive data in notifications; scan uploads; only authorised recipient can see a thread |
| Customers | Verified accounts, guest order links, favourites, alerts, consent state, deletion/export requests | Minimum access; no password viewing; no silent impersonation; audit exports |
| Promotions | Percentage/fixed discounts, product/country eligibility, start/end, min spend, total/per-customer limits, customer-specific code | Server-side validation, stacking policy, price history, currency/tax allocation; public prior-price rules |
| Reviews | Purchase eligibility, moderation queue, reasons, appeal, reply, photo permission | Do not suppress solely for negative rating; product/support/site scores remain separate |
| Health | Release SHA, dependency reachability, queue age, failed webhooks/jobs, reconciliation exceptions, request ID search | Redacted logs, owner role, audited safe retries, no arbitrary SQL/shell or secret display |
| Settings | Legal details, countries, carriers, payment readiness, email sender, support hours, privacy and locale settings | Step-up authentication for sensitive changes; explicit publication workflow |

Roles: **Owner** (finance/settings/access), **Operations** (orders/stock), **Support** (messages and limited order access), **Content** (catalogue/translations/reviews), **Read-only analyst** (aggregates). Verify available engine role granularity during P0. Enforce authorization at every API/action/resource, including object ownership; hiding a navigation item is not security. Start with owner + operations if staffing is tiny, retaining enforceable boundaries.

## 8. Accounts, notifications and feedback

### Accounts and email

Guest checkout is first class. Verify email before linking historic orders; knowing an order number and address is insufficient. Use a maintained auth implementation, secure sessions, rate limits, anti-enumeration and safe recovery. Admin MFA/step-up is mandatory. Customer social login is optional later; never auto-merge solely because an unverified provider email matches.

Transactional emails: verification/recovery, accepted order/payment status, cancellation, dispatch per shipment, delivered/return instructions where carrier evidence exists, refund, support reply, stock alert and post-delivery review invitation. Persist order locale; later browser language changes do not rewrite prior records. SPF/DKIM/DMARC and a shop-specific sender identity are required, even if Piwas reuses an email-service account. Keep mailing streams and suppressions separable from Sofra.

Use versioned templates, durable retries, bounce/complaint handling and preview/testing in all seven languages, including Arabic email clients. No promotional content hidden in transactional mail. Marketing consent and unsubscribe are independent from buying, registering, stock alerts and support.

### Favourites and stock alerts

Local guest favourites, then explicit merge into an account; variant-aware, not only product-level. Out-of-stock alert records a selected configuration, verified address/account, locale and opt-out token. Ask a guest to verify before accepting the alert to prevent abusive subscriptions. Notify only after stock becomes genuinely sellable; deduplicate per restock event and rate-limit. An alert is not a reservation. Stop when it expires or is cancelled, with a proposed 180-day expiry reviewed in the privacy policy. Account users manage alerts without contacting support.

### Feedback design

- **Product:** invite after actual delivery plus a configurable use period (proposed 10 days), with one review per qualifying order item and a truthful “verified purchase” label. No reviews of an unshipped item merely because payment succeeded.
- **Support:** optional satisfaction question after case resolution; private analysis by default. Publish a testimonial only with explicit permission and context.
- **Website:** discreet persistent feedback entry and an occasional optional prompt after task completion. Never interrupt checkout. Ask about task success and what failed, not only a star score.
- Publish honest distribution, count, date, purchased configuration, original language and clearly labelled translations. Product ratings cannot be inflated with support or app feedback.
- Moderate spam, abuse, exposed personal data and irrelevant content with documented reasons; retain critical legitimate reviews. No incentive contingent on a positive review and no “happy customers public, unhappy customers private” routing.
- Empty states say no reviews yet. Structured ratings exist only when eligible real reviews are visibly presented. User photos require permission and safety review; strip location metadata.

## 9. Room experience: appearance and fit are different promises

### Launch: measurement planner + local visual placement

Provide width/depth entry, a measured product footprint, rotation and optional room obstacles. Make diagram controls keyboard-usable with a text equivalent. Show wall distances, approximate required clearance and a printable measurement sheet. Preserve unit labels and clearly state whether dimensions include bolsters/back cushions. Corner geometry must use the actual corner SKU rather than subtracting an invented overlap.

Add **photo placement** once real transparent product cutouts exist: user selects a room image locally, drags/scales/rotates the cutout, optionally calibrates using a known reference length, and compares before/after. Photos remain in-browser by default and leave no server copy. Label this as an appearance preview: a single photograph does not prove perspective, depth, access or physical fit. Do not enable a decorative slider that silently changes a purchased product’s real dimensions.

The current prototype only demonstrates a rectangular fit calculation, not the production photo-placement tool.

### P6: measured 3D and camera AR

Create a dimensionally validated GLB/USDZ for every real supported shape, accurate UV/textile scale and bounded mesh/texture weight. A 3D artist must compare models to physical samples. Use `<model-viewer>`/equivalent with fixed real-world scale, device capability detection, iOS Quick Look and Android/WebXR paths as supported. Request camera access only after the shopper asks to enter AR. Unsupported devices get 3D/photo/2D fallback. [Official AR examples](https://modelviewer.dev/examples/augmented-reality.html)

Test at least supported iPhone Safari and Android Chrome devices with physical tape measurements, floor detection, low light, object placement and pause/resume. AR remains an estimate; doorways, stairs, obstacles, fabric colour, cushion compression and measurement uncertainty need explicit guidance.

### Optional later AI visualisation

Treat generative room styling as inspiration, never proof of fit, texture accuracy or exact included contents. Evaluate privacy/cost, vendor retention, consent, deletion and hallucinated components before adding it. Do not send room photos to a model silently. This is not needed for first sales and should not delay the reliable planner.

## 10. Localization and themes

Preference order: explicit locale URL → saved manual choice → first supported device language → English fallback. Dutch is the initial catalogue/default market language; English is the proposed fallback for unsupported EU languages. A user’s language does not set shipping country, taxes or currency. Avoid geolocation permission prompts for language selection.

Use `/nl`, `/de`, `/fr`, `/en`, `/ar`, `/tr`, `/ku` public routes; internal Kurmanji key can be `kmr`. Render `lang` appropriately, Arabic `dir=rtl`, Kurmanji Latin LTR. Accept `ku`, `ku-TR`, `kmr`, and `ku-Latn` requests intentionally; do not serve Sorani as if it were Kurmanji. Use ISO-639-1 `ku` for Google language annotations; document the dialect in content. [Google language guidance](https://developers.google.com/search/docs/specialty/international/localized-versions)

Localize interface, catalogue, help, policies, error messages, emails and customer notifications, with native review. Maintain translation coverage by route/product version; never advertise a French product page that falls back to English copy. Arabic requires logical CSS properties, mixed-script order IDs, phone/address inputs and screenshots. No flags as language labels. Preserve route, configuration and payment-return parameters when changing language. Browser Back/Forward and cookies-disabled navigation must remain correct.

System/light/dark preference: inline pre-hydration bootstrap or equivalent prevents theme flashes; manual preference persists and system changes apply only in system mode. Price charts, form errors, photos, controls, skeletons and all account/admin surfaces work in both themes. Theme never recolours the product fabric. WCAG 2.2 AA is the engineering target; assess applicable EAA obligations separately. [EAA](https://business.gov.nl/regulations/rules-for-accessibility-eaa/)

## 11. Architecture and implementation boundaries

**Preferred candidate:** independent TypeScript repository with a custom Next.js storefront and Medusa v2 commerce core, PostgreSQL, durable workers/queue, object storage and a customised operations admin. Pin a supported stable dependency set at implementation start; do not clone Sofra’s older/beta versions blindly.

Why: this business benefits from commerce primitives for inventory, reservations, kits and order workflows while retaining full control of design. Medusa documents inventory kits and reservation support. [Inventory module](https://docs.medusajs.com/resources/commerce-modules/inventory)

**P0 proof before committing the stack:** filled/covers-only shared inventory under concurrency; one-off Mollie creation + duplicate webhook + refund; late-paid-after-hold-expiry; country-specific tax/shipping; localized product data; enforceable admin roles. A suitable maintained Mollie provider has not been verified in this research. Budget for a reviewed custom provider adapter; do not assume a plugin solves the contract. Validate Medusa’s admin extension/localization capabilities rather than committing to a fork of its internal UI.

If this proof fails, choose a custom Next.js modular monolith with PostgreSQL and explicit domain services, or Shopify if the owner prefers faster standard operations. Record the concrete trade-off, revised cost and schedule in an ADR. Do not keep two commerce systems or duplicate their stock/order truth. Shopify’s external-gateway fees require current pricing comparison. [Shopify provider fees](https://help.shopify.com/en/manual/payments/third-party-providers)

### Proposed components

- `storefront`: server-rendered routes, product/read models, progressively enhanced checkout; small client islands for configuration/planner/account interactions.
- `commerce`: products/variants, customers, carts, inventory, orders, payments, promotions, taxes and fulfilment through engine services/workflows.
- `extensions`: Mollie provider, country/packaging policies, stock subscriptions, reviews, support cases, translations, content publication and audit access.
- `worker`: payment reconciliation, reservation expiry, email outbox, restock alerts, feed publication, retention jobs and carrier status.
- `admin`: engine operations extended for Piwas; privileged diagnostics isolated from customer-facing APIs.
- `media`: public catalogue derivatives via CDN; private uploads behind signed access and retention policy. Do not mix room photos into public buckets.
- `observability`: structured redacted logs, traces/correlation IDs, metrics, errors, audit trail and uptime checks. No marketing tracker needed for operational metrics.

A CDN/reverse proxy fronts public cached content and private uncached sessions. The storefront must not require the entire 3D runtime or admin bundle to render. Independent database/role, secrets, domain, email identity, deployment and resource limits keep the shop separate from live Sofra/restaurant workloads. No live infrastructure is provisioned during this planning task.

### Conceptual data boundaries

| Domain | Records / important fields |
|---|---|
| Catalogue | Product, Variant, ProductTranslation, MediaAsset, DimensionSpec, ComponentRequirement, SafetyDocument, SupplierBatch |
| Stock | InventoryItem, Location, StockMovement, Reservation, Allocation, Quarantine, PurchaseOrder |
| Commerce | Cart, Quote, immutable Order/OrderLine snapshots, AddressSnapshot, TaxLine, ShippingQuote, PaymentAttempt, ProviderEvent, Refund, CreditDocument |
| Delivery | Shipment, ShipmentLine, Parcel, TrackingEvent, ReturnRequest, ReturnInspection |
| Customer | Account, verified identity, sessions, Address, Favourite, SavedConfiguration, StockSubscription, ConsentRecord |
| Engagement | ProductReview, ReviewEligibility, ReviewMedia, SupportThread, Message, SupportSurvey, SiteFeedback |
| Operations | Promotion, Redemption, PriceHistory, MarketPolicy, TranslationCoverage, Job, OutboxEvent, AuditEvent |

These are conceptual contracts, not instructions to duplicate engine tables. For Medusa, extend/link its modules with the supported persistence model; do not add Prisma to write directly into Medusa-owned tables. If a custom monolith is selected, use one maintained ORM and migrations for its own schema.

External API contracts: quotes and checkout are server mutations; order/return/favourite operations enforce ownership; product publishing invalidates only relevant cache/feed pages; support attachments are private; admin jobs require a narrow action and reason. Rate-limit abuse-sensitive mutations without blocking legitimate provider retries indiscriminately.

## 12. Security, privacy and reliability

- TLS/HSTS, secure HttpOnly sessions, CSRF protection and origin validation, validated input/output, parameterized DB operations, strict upload validation, CSP and output escaping. Sanitize catalogue/rich-text content. Never use user-supplied URLs for unrestricted server fetching.
- Admin MFA, least privilege, session revocation, step-up for refunds/access changes, and auditable exports. No production “debug mode” exposing stack traces, tokens or raw requests.
- Server-owned totals and authorization for every resource; test cross-account IDOR, guest token guessing, promotion abuse and simultaneous last-stock purchases.
- Payment credentials only on the server. No card storage. Confirm the applicable provider/PCI responsibilities; hosted checkout reduces card-data exposure but does not remove all merchant obligations.
- Minimal PII in logs, redacted query strings and request bodies, restricted error access. Audit actor/action/resource/time/outcome separately from diagnostic logging. Safe refund/job retries must not allow arbitrary commands.
- Separate staging/test keys/data. Production data is not copied to preview environments without a defined anonymization process.
- Room photos local-only by default. If cloud saving is later introduced, explicit consent and purpose, signed access, encryption, malware checks, metadata stripping, time-bound retention and deletion including derivatives are required. No model training consent bundled into purchase.
- Privacy notice, processor agreements, retention schedule, access/deletion/export workflows and cookie policy. Strictly necessary storage first; analytics/marketing only under the applicable consent model. No third-party session replay recording addresses/payment forms.
- Business/tax record retention can outlast account deletion; pseudonymize nonrequired fields and explain the distinction. A proposed short diagnostic-log retention (e.g. 30 days) needs final policy approval; it is not the accounting retention period.
- Encrypted backups and restore rehearsal. Initial engineering targets: RPO ≤15 minutes, RTO ≤4 hours for commerce data, monthly restore test. Do not claim these are achieved until measured. Never silently share the restaurant’s production failure domain without capacity/restore review.
- Operational alerts: missing/failed payment reconciliation, unpaid allocation, stock drift, email dead-letter backlog, elevated checkout failures, exhausted storage and backup failure. Owner sees actionable incidents with correlation IDs, not customer secrets.

## 13. Performance and accessibility budgets

Targets, not claims of current performance:

| Measure | Initial release target | Evidence |
|---|---|---|
| LCP / INP / CLS | ≤2.5s / ≤200ms / ≤0.1 at mobile p75 | Field data after traffic, segmented by page/country/device; lab regression checks before launch |
| Initial storefront JS | Aim ≤170 KiB compressed, excluding explicitly opened planner/AR | Build report with named budgets; revise only with evidence |
| Hero image | Responsive AVIF/WebP, typically ≤250 KiB for mobile selected source | Real network transfer, correct sizes/preload, no layout shift |
| Product interaction | Configuration feedback within 100ms for local changes | Browser trace; authoritative quote can update separately |
| Quote API | p95 ≤500ms excluding external carrier/provider latency | Representative load with DB contention and 27-country policies |
| Checkout resilience | No duplicate order/charge/stock deduction in fault scenarios | Integration tests and provider-sandbox reconciliation |
| Accessibility | WCAG 2.2 AA target across purchase and account flows | Automated checks plus keyboard, screen-reader, zoom and RTL review |

Core Web Vitals thresholds follow [web.dev’s definitions](https://web.dev/articles/vitals); the other budgets are proposed engineering constraints.

Cache editorial/product read models, invalidate on publishing and availability changes, and keep stock/price authoritative at quote and payment. Never cache personalized basket/account data publicly. Cache keys include locale, destination/price context where relevant, and content version. Serve meaningful HTML without requiring the configurator bundle. Reserve media dimensions, use font fallbacks/subsets with broad script coverage, lazy-load offscreen assets and defer 3D until requested. Avoid global blur/video/canvas effects and large tracking bundles. Performance measurements use a real representative mobile connection, not a local desktop-only score.

## 14. Public search discovery, SEO and AI discovery

Adapt the existing Sofra runbook’s distinction: **signals served ≠ crawled ≠ indexed ≠ ranked ≠ cited by AI**. Reuse its checking approach, not restaurant schema or tenant-specific assumptions.

### Technical delivery

1. Single canonical hostname and explicit redirect policy. No canonical construction from untrusted incoming Host. Permanent redirects for changed slugs, real 404/410 for removed products where appropriate, useful out-of-stock pages preserved when stock is expected back.
2. Locale-specific URLs with server-rendered title, description, H1, product facts, price/availability, links and `lang`/`dir`. Explicit URLs do not redirect to device language. Root entry negotiates preference without caching one person’s language for everyone.
3. Self canonicals and reciprocal `hreflang` for actually complete equivalents; neutral x-default selection page where useful. Use `ku` for Kurdish search annotation while clearly identifying Kurmanji. No seven-language sitemap entries for untranslated catalogue content.
4. Clean XML sitemap for indexable pages with honest modification timestamps. Account, basket, checkout, payment return, support, admin, previews and room photos/configurations remain outside it. Authentication protects private data; robots.txt is not access control. Let crawlers see `noindex` where needed instead of blocking it accidentally.
5. Variant strategy: stable product family page; each selectable sellable configuration has a shareable resolved URL and SKU. Use supported ProductGroup/Product/Offer structure for genuine variants and `additionalProperty`/visible text for unsupported option semantics such as filling. Choose canonical treatment explicitly so query combinations do not create infinite duplicate pages. Merchant feed URLs preselect the advertised variant and show its exact price/availability.
6. Server-generated Product/Offer, Organization, BreadcrumbList, return/shipping information and genuine eligible Review/AggregateRating data. SKU, dimensions, material, image, price, currency and availability match visible content and merchant feeds. Do not invent GTINs, certification, reviews or brand history. [Google product guidance](https://developers.google.com/search/docs/appearance/structured-data/merchant-listing), [variant guidance](https://developers.google.com/search/blog/2024/02/product-variants)
7. Real crawlable product imagery, descriptive alt text, image sitemap if useful, relevant filenames and correct structured image URLs. Decorative generated interiors are not substitutes for product documentation.
8. Search Console and Bing ownership verification for the new domain; submit sitemap, inspect rendered/raw pages and engine-selected canonicals. Add Merchant Center/free-listing feeds with supported country/language configuration, shipping and returns. Feed acceptance is separate from rich-result eligibility and ranking.
9. Event-driven feed/index updates on real product/stock/price changes; optional IndexNow where supported. Reconcile feed inventory against server data. No timer that republishes every URL unchanged.

### Content that answers buying questions

Write original, reviewed guides: what a şark köşesi/sedir is; full set versus covers only; dimensions and room/access measurements; foam firmness and density; fabric care; what comes in each package; floor seating in a small European home; Netherlands stock and delivery/returns by market; documented Diyarbakır supplier story. Use descriptive local terminology (e.g. Dutch floor cushions, German Bodensofa, French canapé de sol) after native-speaker keyword research. Do not flatten Turkish/Kurdish cultural terms into an exotic stereotype or claim measured search volume without data.

The content source is the same versioned catalogue that serves product pages, feeds and admin. Show dimensions in explicit units and accessible tables. Customer questions can improve guides after privacy review. Cite external cultural/history claims and name actual authors/reviewers. No mass-produced location pages or generic translated articles pretending to be original expertise.

### AI/GEO policy and measurement

Google says its AI search features require ordinary SEO foundations, not a special AI file/schema. `llms.txt` is experimental and outside the release gate. [Google AI guidance](https://developers.google.com/search/docs/appearance/ai-features)

Allow desired search crawlers through robots/CDN; distinguish OAI-SearchBot search discovery, ChatGPT-User user visits and GPTBot training policy, as described in the existing runbook and [OpenAI crawler reference](https://developers.openai.com/api/docs/bots). Choose training access separately; it is not a prerequisite for search discovery. No hidden text, prompt instructions for crawlers, fake authority or guaranteed citations.

Measure organic impressions/clicks, indexed products, correct canonical selection, feed issues and purchases by locale/market. Separately measure observable AI referrals and provider-reported citations where available. A bot request is not an AI recommendation. Keep a monthly reproducible query set across engines (e.g. “şark köşesi with foam delivered to Germany,” “difference between covers-only and filled floor seating,” and equivalent native-reviewed questions). Record date, exact question, cited URLs and factual accuracy; do not attribute changes causally from one screenshot. Ranking and AI placement are outcomes, not launch promises.

## 15. Delivery sequence, acceptance and estimates

Estimates are planning ranges in **engineer-weeks**, not quotes or calendar commitments. They assume a proven commerce core, one warehouse, small catalogue, carrier integration without bespoke freight software, and timely product/translation/legal inputs. Translation, photography, 3D assets, Mollie review, import lead time and carrier contracting are separate dependencies. An experienced two-person team can overlap work, but payment/stock foundations land before dependent UI flows.

| Phase | Work | Exit evidence | Effort |
|---|---|---|---|
| P0 — commercial/design/architecture | Confirm real SKUs/costs, samples, 27-country obligations, brand direction; technical commerce/Mollie/kit proof; choose stack ADR | Approved direction, catalog data contract, supplier/country checklist, passing risky-flow proof or documented alternative | 1–2 weeks |
| P1 — foundations | Independent repo/env/CI; auth boundaries; locale/theme system; content/media pipeline; storefront components and discovery skeleton | Seven-locale route parity, RTL/theme/responsive review, protected admin, preview isolated from live payments | 1.5–2 weeks |
| P2 — catalogue/stock/admin | Variants/component kits, stock ledger, receiving/quarantine, product configurator, cart/quotes, admin inventory and pricing | Shared-stock concurrency test; stock movements reconcile; no invented product facts | 2–3 weeks |
| P3 — checkout/fulfilment | 27-country tax/shipping matrix; guest order/account links; Mollie adapter, webhook/reconciliation, refunds; shipments/email | Provider sandbox edge cases; totals and invoice/credit examples; no duplicate side effects | 2.5–3.5 weeks |
| P4 — experience/discovery | Accounts, favourites/alerts, support/reviews/promos; 2D planner/local photo placement; guides, feeds/schema, diagnostics | Ownership/alert/moderation checks, publish workflow, localized email/commerce, real cutouts and measured footprints | 2–3 weeks |
| P5 — launch readiness | Native language QA, accessibility/performance/security review, operational rehearsal/restore, real product assets and country readiness | Every launch gate below evidenced; controlled live payment/refund/payout only when authorised | 1.5–2.5 weeks |
| P6 — spatial enhancement | Measured 3D assets, AR, saved rooms if justified | Real-device dimensional comparison, fallback/privacy/performance verification | Additional 3–6 weeks plus asset production |

**Core engineering range:** approximately 10.5–16.5 engineer-weeks before contingency. Reserve 20–30% for integration and physical-product uncertainty. A fully custom commerce engine materially increases this; re-estimate instead of hiding it inside the same range. Do not promise a launch date before P0 inputs and Mollie/carrier timelines are known.

### Repository and release discipline

Use a separate app repository with `develop` integration and `main` release branches, PR templates, path-scoped commits in shared clones, pre-commit checks and fail-closed local review. Build/lint/typecheck/tests, dependency/secret scans and the configured review/Sonar gates apply before merge. Use the workspace merge-gate workflow for repositories configured for it; establish equivalent protection before the new repository takes payments. Staging deploys first with test keys and migrations rehearsed. Production migrations, rollout/readback and rollback are explicit release steps. A successful CI run is not a production version or payment-readiness check.

### Required tests

- Unit/property tests for money rounding, discounts, stock availability/component consumption, dimensions and locale negotiation.
- Database integration: two shoppers buy the last shared component; mixed filled/empty orders; hold release vs paid webhook; rollback after process failure; simultaneous refund limits.
- Provider contract: paid/failed/cancelled/pending/expired; duplicate/out-of-order events; unknown IDs; wrong profile/mode/amount/currency; timeout after create; browser never returns; provider returns before webhook; refund retry/reconciliation.
- Browser journeys: guest and account checkout; basket surviving failure; partial shipment/return; all seven locales; Arabic RTL; system/light/dark; desktop/mobile; cookies/storage unavailable; keyboard and screen reader.
- Security: cross-user orders/messages/uploads, privileged actions, CSRF, token expiry, account enumeration, upload abuse and redacted observability.
- Discovery: raw HTML canonical/alternate/schema, missing translation control, stale variant price/stock control, private-route exclusion, expected pages present in sitemap, known-invalid input to prove the verifier discriminates.
- Operational: receiving and packing a real sample set, package/carrier weights, damaged/returned stock quarantine, email failure retry, backups restored to an isolated environment.

### Launch gates

- [ ] Real brand/domain, Piwas identity, provider profile and payout readiness confirmed.
- [ ] Real product photography/specs, components, measurements, packaging and evidence approved.
- [ ] All 27 country policies resolved, including exceptional territories/address handling and return costs; UI languages distinguished from mandatory label/safety languages.
- [ ] Tax/OSS/import/product/packaging/privacy/consumer obligations checked by appropriate owners/advisers; policies actually reflected in code.
- [ ] Inventory concurrency and all payment/refund/recovery cases pass with reconciled outcomes.
- [ ] All seven launch languages reviewed; product/policy/email translations complete; Arabic and Kurmanji checked by native readers.
- [ ] Accessible purchase path and performance budgets pass; no unresolved critical/high security issue in the chosen risk process.
- [ ] Alerts, audit, backups and restoration rehearsed; support and fulfilment owners can handle an exception without a developer editing the database.
- [ ] SEO/feed/raw-output validation passes; Search Console/Bing/Merchant review results recorded separately from delivery status.
- [ ] Owner approves production release; controlled live payment/refund and later settlement readback prove the new shop’s rail.

## 16. Decisions and evidence still needed

Confirmed: Piwas seller, NL stock, EU-wide launch, narrow model focus, with/without foam, seven languages, design discussion before production.

Needed next: actual brand candidates; supplier model list and samples; standard sizes/component quantities; whether stock is assembled or prepacked; foam specifications; intended margin and pricing; packing sizes and carrier quotes; return handling address/capacity; launch inventory budget; photographs/rights; supplier safety/traceability documents; country advice; supported team languages; who will operate support and warehouse.

Design discussion: contemporary courtyard direction versus more textile/people-led imagery; level of ornament; prominence of room planning; how to name the configurations in Dutch and other languages. The current preview gives a concrete basis for those choices. It does not finalise the brand, product specification or production scope by itself.
