FROM rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b AS rust-build
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
ENV CARGO_BUILD_JOBS=1
RUN cargo build --release --locked -p commerce-api

FROM node:24.22.1-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS web-build
WORKDIR /app
COPY package.json package-lock.json .npmrc ./
COPY apps/web/package.json ./apps/web/package.json
COPY packages/design/package.json ./packages/design/package.json
COPY packages/contracts/package.json ./packages/contracts/package.json
RUN npm ci --ignore-scripts
COPY apps/web ./apps/web
COPY packages ./packages
COPY tsconfig.json ./
ENV NEXT_TELEMETRY_DISABLED=1
ENV NODE_OPTIONS=--max-old-space-size=1536
RUN npm run build --workspace @piwas/web

FROM node:24.22.1-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS api
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/piwas-21/sark-kosesi" org.opencontainers.image.revision=$SOURCE_REVISION org.piwas.environment="staging"
WORKDIR /app
COPY --from=rust-build /source/target/release/commerce-api /app/commerce-api
USER node
ENTRYPOINT ["/app/commerce-api"]

FROM node:24.22.1-bookworm-slim@sha256:be8204d4bd327363357f532187b752ff62759c31bedf98e9e18fc08c9cbd605c AS web
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/piwas-21/sark-kosesi" org.opencontainers.image.revision=$SOURCE_REVISION org.piwas.environment="staging"
WORKDIR /app
COPY --from=web-build --chown=node:node /app/apps/web/.next/standalone ./
COPY --from=web-build --chown=node:node /app/apps/web/.next/static ./apps/web/.next/static
COPY --from=web-build --chown=node:node /app/apps/web/public ./apps/web/public
ENV NEXT_TELEMETRY_DISABLED=1 HOSTNAME=0.0.0.0 PORT=3000
USER node
ENTRYPOINT ["node", "apps/web/server.js"]
