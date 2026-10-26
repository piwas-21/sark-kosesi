# Piwas şark köşesi — development rules

Read [ROADMAP.md](ROADMAP.md), [implementation handoff](docs/plans/IMPLEMENTATION-HANDOFF.md), and the owning specification before editing. These rules apply to human and AI contributors. `CLAUDE.md` points here; do not maintain competing rule sets.

## Product and boundaries

- Legal seller: Piwas, Netherlands. Stock in the Netherlands; intended EU-wide delivery. The brand/domain are undecided; Dicle is a working name.
- Narrow model range, with explicit foam and covers-only configurations. Shared physical components cannot become independent fictional stock pools.
- Direction 02, “The living courtyard,” is the design baseline. Preserve its regional character, strong typography, custom icons and purposeful motion. Adapt the design to real content and accessibility; do not copy the prototype into production wholesale.
- `design/prototype/` is a non-selling design specimen. Sample prices, dimensions, reviews/admin data, generated images and draft translations must never become production catalogue facts.
- This is independent of Sofra. Never reuse Sofra keys, databases, customer records, subscription flows or production infrastructure implicitly.
- Preparation is complete when the handoff and foundation checks are verified. Production remains disabled until the explicit release gates pass.

## Work and delivery

1. Use an owned branch/worktree. Never reset, clean, stash or stage another session’s changes. Check status before work; commit only owned paths. Do not use `git add -A` in a shared checkout.
2. Branch from `develop`, using `codex/<purpose>` by default. Open feature/fix/docs PRs to `develop`. Only a release PR from `develop` may target `main`.
3. Never push directly to `main`/`develop`, bypass hooks or rules, force-push protected branches, disable checks, merge with admin bypass, or turn a required check into `continue-on-error`.
4. Install hooks with `npm run hooks:install`. Run `npm run check` and the relevant feature checks before review. Local hooks are assistance, not server protection.
5. Obtain an independent code review before pushing. An available review agent is permitted for that focused task; follow the review rubric in `docs/engineering/DEVELOPMENT.md`. Record the reviewed commit with `scripts/record_review.py`; do not self-approve or fabricate reviewer findings. Re-review after material changes.
6. Merge only through `python3 scripts/merge_gate.py PR --merge`. It requires the configured checks, the current head, current integration base, resolved conversations, and the review receipt. Missing, skipped, stale or empty evidence fails.
7. Squash feature PRs and delete only their feature branches. Release PRs and the required `main → develop` synchronization PR use merge commits; never delete either protected branch. No auto-deploy is configured yet. A merge is not deployment or operational proof.
8. Keep repository rules active. The repository is currently public at the owner’s request. Before changing visibility, run the GitHub protection audit. GitHub Free private repositories lose server-enforced branch protection; this is a production blocker, not an implicit exception.
9. Update the authoritative doc affected by a change. Plans live under `docs/plans/`; finished work moves into code/history, with remaining tasks in ROADMAP. No session journals in product documentation.
10. Routine implementation decisions within the agreed scope do not need repeated permission. Record consequential architecture changes in an ADR. Do not purchase services, create contracts, send customer messages or execute live payments without the applicable explicit authorization.

## Architecture and code

- Target: TypeScript monorepo, server-rendered Next.js storefront, Medusa v2 candidate, PostgreSQL, durable jobs and private/public object-storage separation. ADR-001 is conditional: P0 must prove kits, concurrency, Mollie, late payment, taxes/shipping, translations and admin permissions before engine adoption.
- Node 24.22.1 and npm 11.19.1 are pinned for this foundation. Use maintained stable framework versions verified at P0, exact direct dependency versions and a committed lockfile. Do not silently copy Sofra’s older/beta dependency set.
- Planned layout: `apps/storefront`, `apps/commerce`, `packages/contracts`, `packages/design`, `tests/{unit,integration,e2e,security,discovery,performance}`. Do not create extra services until there is a concrete boundary. Medusa-owned tables have one owner: use its supported modules/workflows, not a second ORM.
- Production TypeScript is strict: no `any`, unexplained non-null assertions, suppressed errors, unchecked casts or unchecked JSON. Validate untrusted data at boundaries. Keep routes/controllers thin and domain logic testable.
- Shared money type: integer minor units + ISO currency. Never trust client totals, stock, roles, ownership or provider-return parameters. Preserve immutable order/tax/discount snapshots.
- Payment attempts have stable idempotency keys. Unknown provider outcomes are reconciled, never replaced blindly. Do not hold a database transaction across provider/network I/O. Confirm payment by server fetch-and-verify through the correct profile.
- Use atomic component reservations and explicit expiry/late-payment recovery. At-least-once jobs require idempotent handlers and an outbox. Refund and restocking are separate operations.
- New storefront components use CSS Modules and shared semantic tokens, with logical CSS properties. `html[data-theme]` owns system/light/dark. Dynamic inline layout values are allowed; hardcoded component colours are not. Prototype CSS is reference material, exempt from production structure rules.
- Accessible primitives centralise buttons, fields, errors, modal focus, navigation and status. Prefer server components and small client islands. No animation library, global state framework or 3D runtime by default.
- Production page components <=200 lines, UI components/hooks/services <=250, domain modules <=300, CSS modules <=250. Split by responsibility; do not minify or move complexity into opaque helpers to satisfy counts. Generated files and explicit test fixtures are exempt.
- Config/URLs/credentials live in validated environment configuration. Never commit secrets or use production resources in tests. `.env.example` contains names and harmless placeholders only.

## Locales, design and content

- Dutch, German, French, English, Arabic, Turkish, Kurmanji: `nl,de,fr,en,ar,tr,kmr` internally. Public Kurdish URL/HTML/search mapping must be explicit and tested; see the discovery spec.
- Explicit locale URL wins, then an explicit saved choice, then device language, then Dutch fallback. Preserve route, selected variant and necessary payment/auth parameters when switching. Never vary a cached explicit-language response by the viewer’s device language.
- Arabic is RTL including forms and dialogs; maintain numeric/product-code direction where needed. Use logical properties and test native readers. Never letter-space Arabic.
- Device theme by default; explicit choice persists and is available from first paint. Storage denial must not break loading.
- No missing-key English fallback presented as a completed translation. Draft locales can be navigable in previews but cannot appear as fully translated indexable equivalents.
- Motion conveys change and remains optional. Respect reduced motion, visible focus, keyboard navigation and 200/400% zoom. No scroll hijacking or product facts hidden in a canvas.
- Real product facts, original/permissioned photos, verified purchase review eligibility and separate support/app feedback. No invented testimonials, safety claims, provenance, GTINs, ratings or performance claims.

## Tests, security and performance

- Follow the test matrix and risk-based strategy. Tests assert externally defined outcomes; do not calculate the expected result with the implementation under test. Add negative and positive controls for “zero findings” instruments.
- Every feature PR includes relevant unit/integration/contract tests. Use real isolated PostgreSQL for locks, transactions and migrations. Mocks cannot prove stock/payment concurrency. Never test against shared development or production databases.
- Critical journeys: guest checkout, asynchronous payment return, component scarcity, order ownership, refunds, restock alerts, locale/theme, RTL and cancellation/retry. CI has no `--passWithNoTests`.
- Foundation CI does not prove commerce readiness. Adding production app files requires activating application CI in the same PR. See the explicit gate matrix; no placeholder green jobs.
- No known unaccepted critical/high vulnerabilities or secrets. CodeQL, dependency audit, secret scan and human threat review are complementary. Any security exception is narrow, time-bounded, recorded and cannot waive ownership/payment integrity.
- Admin MFA/least privilege, server ownership checks, CSRF protections, rate limits, safe uploads, CSP, redacted logs and audit events are mandatory production controls. Debug access never provides shell/SQL execution or raw PII.
- Performance budgets live in `config/performance-budgets.json`. Browser lab evidence does not establish field Core Web Vitals. Image/JS budgets exclude the unoptimised design specimen only, never real store routes.
- SEO/AEO/GEO checks inspect served HTML and domain policy. Distinguish served, crawled, indexed, ranked and cited. Do not claim rankings/citations from a green sitemap test.

## Finish and handoff

Run relevant checks once, repair real failures, verify the exact pushed revision, and leave owned work clean. Report source/CI, provider sandbox, deployed behavior and physical fulfilment separately. Update ROADMAP and the handoff with the next concrete slice and remaining external dependencies. Do not start a separate AI session unless the user asks to create one.
