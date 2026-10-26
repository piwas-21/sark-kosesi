# ADR-001 — Independent commerce foundation

Status: **conditional proposal; implementation proof pending**. Legal company Piwas; independent product, data, deployment and provider profile. No brand commitment.

## Decision boundary

Use a TypeScript monorepo with a custom server-rendered Next.js storefront. Evaluate Medusa v2 for catalogue, inventory, reservations, order/payment/refund and admin extension primitives, with PostgreSQL as durable state and an outbox/worker for external effects. Prefer engine-owned data/workflows; never write a second ledger beside the engine or use an unrelated ORM to edit its tables.

Node 24.22.1/npm 11.19.1 are foundation tooling pins, checked against the official release index. At P0 pin a compatible stable framework set. Medusa v2.22 release notes raise the Node requirement, while some installation pages lag; verify the specific selected release rather than copying a stale starter. [Medusa releases](https://github.com/medusajs/medusa/releases), [Node releases](https://nodejs.org/en/about/previous-releases).

The storefront remains custom so the living-courtyard direction is not constrained by a generic shopping template. Use server-rendered public read models and progressively enhanced controls. No admin/AR dependency in the initial storefront bundle.

## Proof matrix

| Invariant | Executable experiment | Pass criterion | Evidence |
| --- | --- | --- | --- |
| Shared kits | 5 cover kits / 3 foam kits, mixed concurrent requests | At most 5 cover kits and 3 foam kits committed; losers receive recoverable stock response | Pending |
| Last-stock contention | 50 simultaneous requests against one unit in real PostgreSQL | Exactly one successful allocation, no negative stock; idempotent replay does not consume twice | Pending |
| Reservation meaning | Inspect reservation, allocation and on-hand transitions | Document whether allocation is included in reservations; no double subtraction | Pending |
| Unknown creation | Provider accepts then transport times out; client retries | Same stable attempt/key recovered, no second payment/order | Pending |
| Webhook replay/order | Duplicate, delayed, invalid, mismatched profile/amount/mode callbacks | One business transition and one outbox event; mismatches cannot fulfil | Pending |
| Late paid | Hold expires, stock sold, then original payment succeeds | Atomic reallocation or visible finance/refund exception; never silent oversell | Pending |
| Partial refund | Discount + VAT + shipping + multiple quantities | Exact bounded cents and durable state; refund does not silently restock | Pending |
| Market quote | NL/DE/FR plus unsupported postcode and bulky parcel | Server totals and eligibility frozen; unsupported destination fails closed | Pending |
| Translation | Public product and admin content in all requested locales | Complete-equivalent publication distinguishable from UI fallback; RTL support feasible | Pending |
| Admin roles | Operations/support/content accounts call forbidden endpoints directly | Server denies access, not just hidden menus; refunds/exports auditable | Pending |
| Failure recovery | Kill worker between external effect and acknowledgement | Retry converges without duplicate business event | Pending |

A local fake proves application recovery logic; Mollie test mode separately proves provider contracts. Keep those columns separate in the eventual evidence. Model refund/capture support only for enabled payment methods.

## Planned boundaries

`apps/storefront`: public/catalogue, cart, account and planner presentation. `apps/commerce`: engine and extensions (Mollie, market policy, support, review eligibility, stock alerts, content publication). Durable worker may share the commerce codebase but runs separately when its workload requires it. `packages/contracts`: versioned schemas generated or derived from the actual service contract. `packages/design`: tokens and accessible storefront primitives.

Private support/room uploads have signed access and lifecycle deletion; public catalogue media have separate storage keys/CDN policy. Default room-photo placement is local-only. Hosted payments keep card data out of this system. Logs, financial audit and customer records have distinct permissions/retention.

## Alternatives and rejection criteria

If any critical inventory/payment invariant cannot be implemented through supported Medusa extension points, compare a custom Next.js modular monolith with one ORM/transaction model. Re-estimate its larger order/inventory scope. Shopify remains a business alternative requiring explicit cost/provider/design comparison, not a silent fallback. Do not start both engines.

Use an ADR-002 for the proven version set, supported provider adapter decision, DB/job semantics and the retained spike evidence. No speculative microservices or event broker until workload/failure boundaries justify them.
