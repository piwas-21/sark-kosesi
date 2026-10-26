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

Use Node from `.nvmrc`, npm from `package.json`, Python 3.12+ and a virtual environment. Rust is pinned in `rust-toolchain.toml`; the application is a read-only, authenticated P1 preview. Review dependency licences in [DEPENDENCIES](docs/DEPENDENCIES.md).

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

## Application verification

```sh
python3 scripts/install_app_tools.py
export PATH="$PWD/.local/bin:$PATH"
rustup show
rustup toolchain install nightly-2026-10-25 --profile minimal --component llvm-tools-preview
npm run check:rust
npm run audit:rust
npm run typecheck
npm run test:unit
npm run test:integration
npm run test:migrations
npm run test:contracts
npm run test:coverage
npm run build
PLAYWRIGHT_SKIP_BROWSER_GC=1 npx playwright install --with-deps chromium firefox webkit
npm run test:browser
```

DB commands own a disposable digest-pinned PostgreSQL container. Alternatively set
`P1_POSTGRES_BIN` to a local PostgreSQL binary directory; the harness creates a
private temporary Unix-socket cluster and removes only that cluster afterwards.
Existing DATABASE_URL values are replaced by this owned test environment. Browser
commands start and stop their own authenticated API/web processes. Visual checks
require manually approved screenshots; never use automatic baseline updates to
hide a regression. The initial baseline is an explicit acceptance dependency.

For manual preview, provision a separate local database, copy environment names
from `.env.example`, generate a private preview token and start the built Rust API
and Next.js server on the configured loopback ports. The browser uses Basic auth
with username `preview` and that token. Fixtures are seeded only by the test
harness, not by normal application startup. No production environment is accepted.
The worker is a real database readiness probe; durable effects are deferred to P3.
