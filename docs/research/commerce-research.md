# Şark köşesi commerce — research and design rationale

Research date: 23 October 2026. Prepared for Piwas. Brand undecided; **Dicle** is only the preview’s working name. Confirmed operating model: import from Türkiye/Diyarbakır, hold stock in the Netherlands, sell EU-wide. This is qualitative desk research and limited browser inspection, not a conversion-rate benchmark, audited market study or supplier verification.

## What the best reference experiences teach us

| Reference and evidence | Useful pattern | Adaptation for Piwas | Avoid copying |
|---|---|---|---|
| [Tylko storefront](https://tylko.com/nl-nl) visually inspected; [configurator explanation](https://tylko.com/en-se/faq/articles/customisation/what-is-the-configurator) | Room-led imagery, immediate visual configuration, dimensions and price together; community interiors give the product a life beyond the catalogue. | A short configuration sequence: model → size → with foam / covers only. Preserve selection in a shareable URL. Give exact contents and total before purchase. | A huge catalogue, perpetual promotional banners, unconstrained custom dimensions or claiming an untested conversion improvement. |
| [USM configurator](https://www.usm.com/en-uk/how-to-buy/configurator) | Start from an existing model or a new design, recover a saved configuration, and inspect it in a room through AR. | Begin with a curated seating family. Save the selected configuration and return to it on another device. Add measured 3D models after physical samples exist. | Its full modular engineering complexity; a small shop does not need a universal furniture designer. |
| [IKEA Kreativ](https://www.ikea.com/us/en/customer-service/knowledge/articles/43460e1e-dd01-4fe8-86ac-74efecd7743f.html) and [app/web differences](https://www.ikea.com/us/en/customer-service/knowledge/articles/e308d20e-895e-47ce-9aba-2f30c0d98a75.html) | Several entry points: prepared room, dimensions, photographic room, device-dependent scanning. | Offer a no-camera measurement planner first, followed by measured AR and optional photo visualisation. Always keep a fallback. | Treating room scanning as universally available. IKEA’s published pages differ about current photo-upload availability; do not promise parity based on that evidence. |
| [Ferm Living Rico product page](https://fermliving.com/nl-nl/products/rico-sofa-2-grain-caramel), visually and textually inspected | Large photography, restrained product controls, fabric sample link, delivery estimate, specifications, dimensional drawing and care information. | Add textile close-ups, a sample service if operationally viable, foam density/firmness, all cushion dimensions, and postcode-specific delivery beside the purchase controls. | Its prices or lead times. Those vary by product and market and tell us nothing about Piwas economics. |
| [Alkapida’s category example](https://www.alkapida.com/tr-nl/p/md-home-carpet-sark-kosesi-minder-takimi-yikanabilir-fermuarli-sert-cokmeyen-sungerli/1637850) | The category depends on detailed package contents and individual cushion sizes. The fetched page exposes several delivery statements and an out-of-stock state together. | One authoritative availability/delivery state per selected configuration. Show an illustrated packing list with explicit exclusions. | Treating a marketplace seller’s claims, material quality, delivery promises or prices as independently verified. This is a category reference, not a “top-tier UX” endorsement. |

The useful distinction is **editorial discovery, precise configuration, familiar checkout**. Make the shopping environment memorable, while keeping stock, delivery, returns and payment unambiguous. The first collection should feel deliberately small, not like an empty general furniture store.

## Proposed visual language

Contemporary Diyarbakır: basalt architecture, madder-red woven geometry, mineral plaster, low seating, intimate scale and shared tea. Use real supplier/product/cultural evidence when available. Do not invent motif meanings, artisan biographies, handwoven production, natural fibres, certificates or sustainability claims.

The preview combines a large typographic invitation with an immersive interior. Typography and imagery carry the atmosphere; a small diamond motif provides continuity. Product controls are quiet and practical. A darker basalt palette is equally deliberate. Arabic layouts mirror navigation and reading flow; Kurmanji in Latin script stays left-to-right. No automatic audio, scroll hijacking or moving purchase buttons.

Alternative directions for discussion:

- **Contemporary courtyard — recommended:** red/basalt, space, modern rooms; broadest route to diaspora and design-minded customers.
- **Textile atelier:** stronger close-up weaving imagery, pattern samples and material story; depends on supplier photography and documented process.
- **Family gathering:** documentary homes and people, warmer conversational copy; depends on permissioned real photography and customer stories.

These are creative alternatives, not tested audience preferences. The current interactive prototype develops the first direction. No name or design is approved yet.

## Mollie: account reuse is possible; credentials are separate

Mollie allows multiple website profiles within one organisation when they share the same legal business registration. Each website requires its own profile and API key, with website review. Piwas is the intended seller, so a separate shop profile under the existing Piwas organisation is the appropriate route, once the existing organisation’s legal identity is confirmed. It does not mean reusing Sofra’s live key. [Mollie website profiles](https://help.mollie.com/hc/en-us/articles/212613429-Can-I-add-or-change-a-website-profile)

A different company registration would require a separate organisation, which can still be managed from the same login. [Mollie organisations](https://help.mollie.com/hc/en-gb/articles/360014268520-Can-I-add-another-company-to-my-account)

Local evidence:

- `sofra/lib/mollie.ts:26` reads one `MOLLIE_API_KEY`; line 103 creates a **first** payment that establishes a recurring mandate.
- `sofra/lib/billing.ts` is tenant subscription billing, not goods fulfilment.
- `sofra/app/api/webhooks/mollie/route.ts:35` re-fetches the payment rather than trusting the posted body.
- `sofra/CLAUDE.md` describes a live key, but the historical root roadmap contains a conflicting pending-key statement. Neither is current account-status proof.
- No usable local `MOLLIE_API_KEY` was found in the checked `.env.local` / `.env` files. No account readiness was verified, and no provider or production configuration was changed.

The shop needs one-off payments, independent order records, refunds, delivery costs, stock holds and recovery. Reuse tested integration ideas, not the subscription state machine. Mollie’s current API provides payment metadata, hosted checkout URLs, webhook URLs and payment-method selection. Its documented hosted-checkout locale list includes Dutch, German, French, English and Turkish, but does not list Arabic or Kurmanji. Use an explicit supported-language fallback during handoff, then restore the shopper’s language on return. [Create payment](https://docs.mollie.com/reference/create-payment)

## Platform trade-off

| Approach | Why consider it | Principal cost/risk | Recommendation |
|---|---|---|---|
| Shopify with a bespoke theme | Shortest route to ordinary catalogue/order operations and managed commerce infrastructure. | More constraints around checkout, languages, custom operations and room tooling; apps accumulate. External payment gateways can incur Shopify fees in addition to provider fees. | Strong fallback if speed and lower engineering ownership outweigh deep custom control. [Shopify payment providers](https://help.shopify.com/en/manual/payments/third-party-providers) |
| Next.js storefront + Medusa commerce core | Full visual freedom with existing inventory, reservations and inventory kits, avoiding a brand-new ledger. | New operational stack; Mollie integration, authorization and advanced admin extensions must be proven. | Preferred candidate, subject to the short technical proof defined in the plan. [Medusa inventory](https://docs.medusajs.com/resources/commerce-modules/inventory) |
| Fully custom Next.js/PostgreSQL commerce | Familiar technologies and complete control; useful Sofra patterns already exist. | Piwas becomes responsible for every payment, reservation, return, promotion and audit edge case. | Do not choose only because a Mollie helper already exists. Higher build and maintenance allowance. |
| Fork Sofra or add a restaurant tenant | Superficial infrastructure reuse. | Subscription, restaurant, customer and operational models would leak into a separate retail business. | Reject. Separate repository, database, credentials, deployment and brand. |

The visual direction does not depend on which commerce engine wins. The first implementation milestone must prove the highest-risk engine flows, not just render a starter storefront.

## EU-wide business requirements that affect the app

These are planning requirements to validate with Piwas’s accountant, importer/adviser and carriers before sales, not a completed country-by-country legal opinion.

- **Returns:** online goods generally have a 14-day withdrawal period. Pre-contract information must include return responsibility and an estimate for goods too bulky to return normally by post. Ordinary stock choices should not automatically be labelled “custom, no returns.” [EU distance selling](https://europa.eu/youreurope/business/selling-in-eu/selling-goods-services/ecommerce-distance-selling/index_en.htm)
- **VAT:** model destination VAT and Union OSS. The €10,000 threshold is not a per-country or per-brand allowance; applicability needs company-level assessment, including relevant existing activity. Stock imported into the Netherlands then sold within the EU is a different flow from individual direct-import parcels. [Cross-border VAT](https://europa.eu/youreurope/business/finance-and-tax/vat/cross-border-vat/index_en.htm), [OSS](https://europa.eu/youreurope/business/finance-and-tax/vat/one-stop-shop/index_en.htm)
- **Imports:** determine correct product classification, EORI, customs handling and landed cost. An A.TR may affect duty eligibility; it is not a universal promise of zero import costs or proof of manufacture. [A.TR guidance](https://business.gov.nl/international/doing-business-abroad/a.tr-certificate-for-turkiye/), [customs declarations](https://business.gov.nl/regulations/declaring-goods-customs/)
- **Product safety:** Piwas’s importer responsibilities include verified product documentation, traceability and response to unsafe goods. Own-brand changes can alter its legal role. [Importer safety guidance](https://business.gov.nl/products-services-and-innovation/product-safety/product-safety-for-importers/)
- **Textiles:** verify the applicable fibre-composition label and required market languages. Seven website languages do not cover every EU label requirement. [EU textile labels](https://europa.eu/youreurope/business/product-rules-compliance/textiles-and-footwear/textile-label/index_en.htm)
- **Packaging:** record packaging material/weight and establish applicable producer-responsibility arrangements in destination markets. Netherlands compliance alone is not a completed EU-wide assessment. [Dutch packaging rules](https://business.gov.nl/regulations/packaging/)
- **Discounts:** retain variant/country price history so public reduction claims can use the applicable prior-price basis, generally the lowest prior 30-day price. [Price labelling](https://business.gov.nl/regulations/product-price-labelling/)
- **Accessibility:** aim for WCAG 2.2 AA regardless of any applicable small-business exemption; assess EAA scope for Piwas as a company. [Accessibility rules](https://business.gov.nl/regulations/rules-for-accessibility-eaa/)

## Discovery and room-visualisation evidence

Build product facts into server-rendered pages. Use accurate Product/Offer data and supported variant grouping rather than hoping a JavaScript configurator or a generative room picture can explain the catalogue to a crawler. [Google merchant listings](https://developers.google.com/search/docs/appearance/structured-data/merchant-listing), [product variants](https://developers.google.com/search/blog/2024/02/product-variants)

Google says its AI features use the usual search fundamentals; there is no special required “AI SEO” file or schema. Useful dimensions, delivery facts, original photography, honest reviews and clear guides are a better foundation than invented GEO scores or mass-produced city pages. [Google AI search guidance](https://developers.google.com/search/docs/appearance/ai-features)

Use reciprocal language links with supported language codes. Public Kurdish URLs can use `ku` with reviewed Kurmanji content; the internal translation key may remain `kmr`. Document the mapping and test `ku`, `ku-TR`, `kmr`, and Latin-script preferences. [Google language annotations](https://developers.google.com/search/docs/specialty/international/localized-versions)

For actual AR, a web-based `<model-viewer>` implementation can use GLB and USDZ with WebXR, Scene Viewer or Quick Look and fixed scale. Device support varies; fixed scale still does not certify real-world fit. [Official model-viewer examples](https://modelviewer.dev/examples/augmented-reality.html)

## Evidence limits

No real supplier quote, fabric test, foam specification, warehouse cost, carrier tariff, trademark search, demand measurement, Mollie dashboard status or live payout was obtained. No claim about superior conversion, final price, ranking or AI citation can be inferred. Prototype content uses clearly marked assumptions. These missing commercial inputs are explicit Phase 0 deliverables in the development plan.
