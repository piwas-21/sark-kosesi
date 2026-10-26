# Public search and answer-engine discovery

This adapts the Sofra public-search-discovery approach to a single retail business. Do not import Restaurant schema, tenant identities or restaurant routes. **Signals served, crawled, indexed, ranked and cited are different evidence levels.** No ranking, AI inclusion or sales claim follows from a green build.

## Content and URL contract

- One configured canonical host. Redirect aliases; never derive absolute canonicals from an untrusted Host header. Keep slug redirect history and use genuine 404/410 where appropriate.
- Locale-specific server-rendered URLs, with visible title, H1, description, product/variant facts, price/currency/availability, dimensions, care, delivery/returns and links before hydration. Decorative canvas or AI imagery cannot carry the only product explanation.
- Internal locales: `nl,de,fr,en,ar,tr,kmr`. Public Kurmanji policy: `/ku/` URLs, `lang="ku-Latn"`, `hreflang="ku"`, explicitly described as Kurmanji; preserve an internal `kmr` translation key. Map `kmr`, `ku`, `ku-Latn` and region tags in tests, without pretending Sorani is supported. Arabic has `dir="rtl"`.
- Explicit locale URL wins. Unprefixed entry negotiates explicit stored preference, supported browser language, then Dutch. Do not cache one viewer’s negotiation for everyone. Explicit locale pages remain stable for crawlers and shared links.
- Self canonical per indexable locale and reciprocal alternates only for complete equivalent content. Key parity is insufficient: missing product description/material/care or legal translation cannot be advertised as a complete locale. Draft-language previews remain noindex.
- Use a neutral root selector for x-default if introduced. Never attach every possible language to every page blindly. Language switching preserves route, variant and required checkout/auth parameters.
- Stable product-family landing page and shareable resolved variants. ADR-002 must settle canonical family/variant treatment. Crawlable variant links and feed landing pages must preselect the exact advertised SKU/fill/size, price and availability; filling is not an invented schema vocabulary.
- Expected-to-return out-of-stock products retain useful pages and alerts. Delisted products follow a clear redirect/410 policy, not a home-page redirect for everything. Do not index sort/filter/session/coupon combinations or private saved-room configurations.

## Structured facts and merchant feeds

Use supported ProductGroup/Product/Offer, Organization and BreadcrumbList where the visible facts warrant them. Express dimensions/material/filling in visible tables and appropriate supported properties/additionalProperty. Shipping and return policy data must match the actual market/parcel policy. Product/Offer prices, stock, currency and variant URLs must agree with the catalogue API and merchant feed. Do not invent GTINs, manufacturer certification, brand history or reviews.

Only eligible, actually displayed product reviews can contribute to product AggregateRating. Support and site feedback are separate signals. Do not use schema to turn a private survey into a product review. No fake reviews, review gating, hidden keywords or crawler-targeted persuasion.

Publish authentic product imagery with stable public derivative URLs, descriptive alt text, correct dimensions and rights records. Generated inspiration scenes must remain labelled and cannot substitute for product evidence or a merchant’s required product image.

## Automated discovery acceptance (P1 skeleton, P5 complete)

The checker fetches raw HTML, response headers, robots, sitemap and linked equivalents from the built app. Use Googlebot and an HTML-limited crawler as separate metadata cases when the framework streams metadata; test complete served output, not a mistaken assumption that all metadata arrives in the first head chunk.

| Test | Positive and negative controls |
| --- | --- |
| Expected routes | Known published NL family/variant/guide must be present; deliberately omitted expected route must fail even if every emitted URL is valid |
| Canonical | Exactly one absolute self canonical; reject alias/preview/foreign host, relative/multiple/missing canonical |
| Language cluster | Correct lang/dir, reciprocal self/equivalent links, no incomplete translation; remove one reverse link and require failure |
| Server content | JS-disabled HTML contains actual catalogue facts and actionable links; reject empty hydration shells and placeholder facts |
| Product schema/feed | Parse JSON-LD; compare SKU/options, price minor units/currency, availability and landing variant to the authoritative fixture; inject one mismatch |
| Private routes | Account/cart/checkout/payment return/admin/support/saved rooms absent from sitemap, correct noindex and authentication; test localized roots and descendants |
| Preview policy | Authentication or access control plus noindex; production hostname only when explicitly enabled; no public indexing from preview builds |
| Lifecycle | Product publication/update/stock/deletion updates affected pages/feed/sitemap with honest lastmod; unrelated URLs unchanged |
| Navigation | Locale survives browse/product/cart/account/back-forward/reload and unavailable storage; selecting/preloading an alternate does not silently save a preference |
| Host/cache isolation | Forged Host cannot alter canonical; another customer’s locale/private cart/quote cannot leak through cache |

Robots.txt is not authorisation. A crawler must be able to fetch a public noindex response to observe it; do not block that fetch and then claim noindex was processed. Protect private resources with authentication regardless of robots. Keep expected route inventory separate from the sitemap to detect missing pages.

## Answer-focused content

Create original reviewed guides and useful concise answers: what a şark köşesi/sedir is; filled set versus covers only; actual seat/back/parcel dimensions; foam firmness/density; fabric/care; delivery access and bulky returns by country; stock location; supplier/provenance story supported by evidence. Use headings, accessible tables, units and direct answers, with author/reviewer and honest updated dates.

Research Dutch/German/French/English/Turkish/Arabic/Kurmanji buying terms with native speakers. Keep culturally accurate names and meanings; do not fabricate search volumes, mass-produce city pages or flatten regional craft into stereotypes. Approved catalogue facts are shared by the store, feed, help content and admin. Customer questions can inform guides after privacy review.

Google states ordinary SEO fundamentals apply to its AI search features; there is no required special AI file/schema. `llms.txt` is optional experimentation, outside the release gate. [Google AI guidance](https://developers.google.com/search/docs/appearance/ai-features).

Crawler purposes remain separate: OAI-SearchBot is search discovery, ChatGPT-User is user-requested retrieval, and GPTBot is training policy. Decide training access separately; do not infer it from search consent. Verify current definitions before setting production robots/CDN rules. [OpenAI crawler reference](https://developers.openai.com/api/docs/bots).

## External launch and measurement

1. Verify the chosen domain independently in Google Search Console and Bing Webmaster Tools. Preserve DNS ownership records. Submit sitemap and inspect actual NL home/product plus one complete translated equivalent.
2. Record fetched/indexed state, exclusion reason, user/engine-selected canonical and crawl date. A successful live fetch or sitemap submission is not index acceptance.
3. Configure Merchant Center/free listings for actual countries/languages, shipping/returns and valid variant feeds. Feed approval is distinct from search ranking and checkout availability.
4. Use event-driven feed updates and optional IndexNow only for real publish/change/delete events. Do not rebroadcast every URL on a timer as an optimisation claim.
5. Track qualified impressions/clicks, indexed catalogue, canonical errors, feed disapprovals and conversion by country/locale. Operational analytics and marketing consent are distinct.
6. Keep a monthly query set across Google, Bing/Copilot, ChatGPT search and another relevant answer engine. Record exact question, language, engine/mode/date, cited URLs and factual errors. Observe referrals and provider-reported citations separately. A bot request or referral does not prove an AI recommendation.

Initial query intents: “şark köşesi with foam delivered to Germany”, “covers-only floor sofa dimensions”, “sedir stock Netherlands”, care, contents and return costs. Translate/review the questions, keep the exact set reproducible and do not interpret noisy changes as causal proof.

Primary references: [Google merchant listings](https://developers.google.com/search/docs/appearance/structured-data/merchant-listing), [product variants](https://developers.google.com/search/docs/appearance/structured-data/product-variants), [multilingual annotations](https://developers.google.com/search/docs/specialty/international/localized-versions), [sitemaps](https://developers.google.com/search/docs/crawling-indexing/sitemaps/build-sitemap), [Bing webmaster tools](https://www.bing.com/webmasters/). Source inspiration: Sofra’s shared public-search-discovery runbook, retained in its own repository; this document owns the retail policy.
