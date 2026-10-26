# Piwas şark köşesi

A Piwas-owned shop for a focused Diyarbakır floor-seating collection, stocked in the Netherlands and intended for all 27 EU countries. Brand/domain pending. Production is disabled.

Start with [AGENTS.md](AGENTS.md) and [ROADMAP.md](ROADMAP.md). The [implementation plan](docs/IMPLEMENTATION.md) is the handoff for the next session. [Architecture](docs/ARCHITECTURE.md) defines the selected stack: Rust commerce API and workers, PostgreSQL, Next.js/React storefront and admin, Tailwind semantic tokens and CSS Modules for custom art/motion. No paid commerce engine or managed platform is required.

- [Product and launch inputs](docs/PRODUCT.md)
- [Quality and test gates](docs/QUALITY.md)
- [Security baseline](docs/SECURITY.md)
- [SEO / AEO / GEO](docs/DISCOVERY.md)
- [Operations and release](docs/OPERATIONS.md)
- [AI contribution workflow](docs/AI-WORKFLOW.md)
- [Approved design](design/README.md)

## Work locally

Use Node from `.nvmrc`, npm from `package.json`, Python 3.12+ and a virtual environment. Rust is pinned in `rust-toolchain.toml`; application crates are created in P1, not replaced by dummy passing tests.

```sh
npm ci --ignore-scripts
python3 -m venv .local/venv
.local/venv/bin/pip install --require-hashes -r requirements-dev.txt
export PATH="$PWD/.local/venv/bin:$PATH"
npm run hooks:install
npm run check
npm run preview
```

The prototype is at `http://127.0.0.1:48763/studio.html`. It is a design specimen with sample data and concept imagery, not a store. Browser tests run with `npm run test:prototype` in the pinned CI image. The SonarQube CI job runs a disposable Community Build server; no SonarCloud account or paid subscription is needed. See [quality](docs/QUALITY.md) for the precise evidence boundary.
