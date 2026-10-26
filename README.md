# Piwas şark köşesi

A focused EU furniture shop: Diyarbakır-inspired floor seating, stocked in the Netherlands, sold with foam or as covers only. Piwas is the legal company; the brand is undecided. **Dicle is a working design name.**

This repository contains the product research, development plan, design prototype and implementation foundation. **It is not a working commerce application.** No live checkout, account system, inventory, email, shipment or AR service is connected.

## Start here

1. [Development rules](AGENTS.md)
2. [Current roadmap](ROADMAP.md)
3. [Next-session implementation handoff](docs/plans/IMPLEMENTATION-HANDOFF.md)
4. [Complete commerce plan](docs/plans/COMMERCE-PLAN.md)
5. [Architecture decision and proof](docs/architecture/ADR-001-COMMERCE-FOUNDATION.md)
6. [CI and enforcement matrix](docs/engineering/CI-GATES.md)

## Setup

Use Node **24.22.1**, npm **11.19.1**, Python 3.11+ and Git. `nvm install && nvm use` follows `.nvmrc` when nvm is available.

```sh
npm ci --ignore-scripts
npm run hooks:install
npm run check
npm run preview
```

Open [the local design studio](http://localhost:48763/studio.html). Set `PORT` if needed. The server binds to loopback and serves only the prototype directory. Do not expose it as a public shop.

`npm run test:prototype` runs the browser regression suite after installing Playwright’s pinned Chromium runtime with `node node_modules/@playwright/test/cli.js install chromium`. CI uses the matching official Playwright container, pinned by digest, with browsers and system dependencies already installed. Tests are scoped to a local fixture, never a live provider.

## Specifications

| Area | Source of truth |
| --- | --- |
| Engineering and review | [Development](docs/engineering/DEVELOPMENT.md) |
| Security and threat boundaries | [Security baseline](docs/security/BASELINE.md) |
| Security disclosure | [Security policy](SECURITY.md) |
| Test scenarios | [Test strategy](docs/quality/TEST-STRATEGY.md) |
| Performance and accessibility | [Performance baseline](docs/quality/PERFORMANCE.md) |
| SEO and answer-engine discovery | [Discovery strategy](docs/discovery/SEO-AEO-GEO.md) |
| Release, backup and rollback | [Release runbook](docs/runbooks/RELEASE.md) |
| Product, supplier and market inputs | [Launch inputs](docs/product/LAUNCH-INPUTS.md) |
| Visual direction and media rights | [Design index](design/README.md) |
| Research sources | [Commerce research](docs/research/commerce-research.md) |

## Delivery

`develop` is the integration/default branch. Feature PRs target `develop`; release PRs run `develop → main`. The owner requested public visibility for setup. Visibility changes require a fresh protection audit; GitHub Free does not protect private-repository branches. No automatic deployment exists yet.

Foundation checks cover documentation/assets, gate tooling, prototype behavior, secrets, dependency vulnerabilities and CodeQL. Production gates are explicitly pending; `npm run release:check` fails until their evidence and the application exist. See the CI matrix for what is and is not enforced. Never describe this foundation’s green CI as a production security, performance, translation or commerce acceptance.

Original planning and prototype files have been consolidated here. Old local conversation paths retain compatibility links; this repository is authoritative. Public visibility grants no project reuse licence; see [LICENSE](LICENSE) and [media provenance](design/README.md).
