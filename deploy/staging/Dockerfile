FROM rust:1.99.1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b AS rust-build
WORKDIR /source
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
ENV CARGO_BUILD_JOBS=1
RUN cargo build --release --locked -p commerce-api --bins

FROM node:24.22.3-bookworm-slim@sha256:dc26ae92c9fb15cf6b1d63e20f0944b2efe0f115b20e00be48fb23fceb061c54 AS node-runtime

FROM node-runtime AS web-build
WORKDIR /app
COPY package.json package-lock.json .npmrc ./
COPY apps/web/package.json ./apps/web/package.json
COPY packages/design/package.json ./packages/design/package.json
COPY packages/contracts/package.json ./packages/contracts/package.json
COPY packages/room-fit/package.json ./packages/room-fit/package.json
COPY packages/room-draft/package.json ./packages/room-draft/package.json
RUN npm ci --ignore-scripts
COPY apps/web ./apps/web
COPY packages ./packages
COPY tsconfig.json ./
ENV NEXT_TELEMETRY_DISABLED=1
ENV NODE_OPTIONS=--max-old-space-size=1536
RUN npm run build --workspace @piwas/web

FROM debian:trixie-slim@sha256:a72b4433de1cf7d6bae0c4cee74d0547f58601a643b3375f070c51600d63b6b2 AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends zlib1g=1:1.3.dfsg+really1.3.1-1+deb13u1 \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 1000 node && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash node

FROM runtime AS api
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/piwas-21/sark-kosesi" org.opencontainers.image.revision=$SOURCE_REVISION org.piwas.environment="staging"
WORKDIR /app
COPY --from=rust-build /source/target/release/commerce-api /app/commerce-api
USER node
ENTRYPOINT ["/app/commerce-api"]

FROM runtime AS callback-edge
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/piwas-21/sark-kosesi" org.opencontainers.image.revision=$SOURCE_REVISION org.piwas.environment="staging"
WORKDIR /app
COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node-runtime /usr/local/LICENSE /usr/local/LICENSE
COPY --from=rust-build /source/target/release/commerce-callback-edge /app/commerce-callback-edge
USER node
ENTRYPOINT ["/app/commerce-callback-edge"]

FROM runtime AS web
ARG SOURCE_REVISION
LABEL org.opencontainers.image.source="https://github.com/piwas-21/sark-kosesi" org.opencontainers.image.revision=$SOURCE_REVISION org.piwas.environment="staging"
WORKDIR /app
COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node-runtime /usr/local/LICENSE /usr/local/LICENSE
COPY --from=web-build --chown=node:node /app/apps/web/.next/standalone ./
COPY --from=web-build --chown=node:node /app/apps/web/.next/static ./apps/web/.next/static
COPY --from=web-build --chown=node:node /app/apps/web/public ./apps/web/public
ENV NEXT_TELEMETRY_DISABLED=1 HOSTNAME=0.0.0.0 PORT=3000
USER node
ENTRYPOINT ["node", "apps/web/server.js"]
