# Performance and accessibility baseline

`config/performance-budgets.json` is the machine-readable budget. These targets apply to the production store, not the unoptimised raster design specimen. Do not advertise prototype performance as measured production behavior.

## Budget and measurement

| Measure | Target | How to prove it |
| --- | --- | --- |
| LCP / INP / CLS | <=2.5s / <=200ms / <=0.1 at p75 | Field RUM after sufficient traffic, segmented mobile/desktop and core route; report sample window and count |
| Mobile lab | Lighthouse score >=90, LCP <=2.5s, TBT <=200ms, CLS <=0.1 | Pinned browser/tool, production build, cold navigation, 3-run median with fixed mobile network/CPU profile |
| First-party initial JS | <=170KiB gzip | Route bundle/network report, shared chunk counted once; exclude only explicitly opened planner/AR |
| Mobile LCP image | <=250KiB selected transfer | Actual `srcset`/sizes request; AVIF/WebP derivatives; explicit dimensions and preload policy |
| Fonts / initial mobile page | <=150KiB fonts / <=750KiB total | Cold-cache network including required fonts, CSS, JS and imagery |
| Local configuration response | <=100ms | Browser trace and observable state; server quote may settle separately |
| Catalogue / quote p95 | <=300ms / <=500ms excluding external providers | Fixed dataset, 25 concurrent virtual users for 10min; 50-way stock burst separately |
| API errors | <1% under stated supported load | Distinguish expected validation/conflict responses from server errors; no lost orders |

Core Web Vitals use field thresholds; TBT/Lighthouse are lab diagnostics, not an INP substitute. [web.dev definitions](https://web.dev/articles/vitals). Numeric payload/API/load limits above are this project’s engineering choices and require evidence before adjustment.

Fixtures: one realistic product family with size/fill variants, 1,000 orders, 100 simultaneous holds, seven locales and 27 market policies; seed deterministic data and include both cold/warm caches. Measure on the selected production-like instance and report hardware/region. No load test against live Sofra or payment providers.

## Architecture controls

Server-render product facts. Keep configurator/account/planner as small client islands; lazy-load 3D only after user intent. Cache public read models with locale/market/content-version keys; never share personalised account/cart/quote responses through public CDN caching. Quote/payment always revalidate authoritative price/stock. Product publication and stock changes invalidate matching page/feed data.

Use responsive image derivatives, local subset fonts with Arabic/Turkish/Kurmanji coverage, reserved media dimensions and sensible fallbacks. Avoid global animated canvas/video/blur and third-party trackers in the critical path. Animate opacity/transform; pause offscreen work and respect reduced motion. Use measured interaction traces for any richer effect.

Database indexes follow actual reads and query plans. Watch N+1 queries, unbounded includes/pagination and lock contention. Bound provider timeouts and isolate their latency. Browser measurements do not excuse server data leakage from overly broad caching.

## Accessibility acceptance

WCAG 2.2 AA is the product target. Automated axe serious/critical findings block affected core flows. Manual review covers keyboard-only purchase/account/admin, focus containment/restoration, screen-reader forms/status, contrast, 200%/400% zoom, reduced motion, mobile touch targets, high contrast and Arabic RTL. Test 320/390/768/1280/1440px with long German/French strings and real catalogue names, not only short fixtures.

Provide text/measurement alternatives for visual room tools and photos. Errors identify the field and recovery step without clearing the basket. Loading and pending payments have accessible statuses; no spinner that implies payment succeeded. Every motion treatment must remain usable when animation is disabled.

## Regression policy

Budget jobs become mandatory in P1 and operate on the app build. Store per-route baseline reports; fail explicit limits, and investigate >10% payload/latency regressions even when below the ceiling. Rerun a noisy benchmark with a stated reason, not until it passes. Waivers require owner, exact route/metric, rationale and expiry, with no blanket “temporary” exception.
