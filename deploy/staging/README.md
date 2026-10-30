# Private staging preview

This deploys the non-selling preview for P3 integration work. It does not release
`main`, configure a live payment key or establish provider/product acceptance.
Read AGENTS.md and the operations/security requirements first.

## Isolation and credentials

Use a dedicated deployment directory and Compose project. The database/volume,
service names and generated URL-safe database password/preview token belong only
to this preview. Do not copy another application's environment or database.
Keep the actual environment file outside Git with mode 0600. The committed
`.env.example` contains placeholders only. No Mollie key is required for startup.
Never put credentials in image build arguments, labels, source, logs or artifacts.

The deployment uses the existing `coolify` TLS proxy network. The web/API images
run as UID 1000 with dropped capabilities, read-only roots, bounded temporary
storage, process limits and memory/CPU caps. PostgreSQL has its own internal
network and persistent volume and no published port. The API shares the web
network namespace and listens only on 127.0.0.1:45671. The web's debug port is
published only on host loopback. Only its authenticated ingress can be routed.
No expiry worker, admin authority, merchant data or test fixtures are seeded.

## Build and start

1. Use accepted `develop` source whose complete tree matches the independently
   reviewed feature and all nine current-head gates. Keep `productionEnabled=false`.
2. Resolve the staging hostname to the intended server and verify the existing
   proxy/network before creating any resources. Do not restart or reconfigure
   existing applications. Set the hostname and exact accepted commit in a private
   environment file; generate independent passwords/tokens and leave ingress false.
3. Build immutable local image tags from that exact source, sequentially. Rust
   compilation uses one job and the web build has a bounded heap. Retain build
   results and image IDs; do not pull an unreviewed mutable application image.

```sh
docker build --file deploy/staging/Dockerfile --target api --build-arg SOURCE_REVISION=REVISION --tag sark-kosesi-api:REVISION .
docker build --file deploy/staging/Dockerfile --target web --build-arg SOURCE_REVISION=REVISION --tag sark-kosesi-web:REVISION .
docker compose --env-file PRIVATE_ENV --file deploy/staging/compose.yml config --quiet
docker compose --env-file PRIVATE_ENV --file deploy/staging/compose.yml up --detach --wait
```

4. With ingress disabled, verify both images/processes and the dedicated database.
   The API applies the committed forward migrations. The web health check requires
   a genuine 401 challenge plus API readiness, without disclosing the preview token.
5. After the exact-source checks and private startup pass, enable only this
   project's ingress flag and recreate its web/API services. The host rule must be
   the chosen staging hostname and the TLS resolver must be `letsencrypt`. Verify a
   valid certificate without disabling TLS verification. No public database/API port
   or existing router may be added/changed.
6. Store the preview token by itself in a private local file and run the read-only
   acceptance check. It rejects wrong/missing credentials, incorrect served source,
   unsafe redirect origins, missing privacy headers, wrong RTL and enabled commerce.

```sh
node tests/application/staging-smoke.mjs --origin https://STAGING_HOST --revision REVISION --token-file PRIVATE_TOKEN_FILE
```

This is preview acceptance, not actual order/payment/load/fulfilment or production
acceptance. Use only a dedicated test profile/key for later Mollie work. Keep live
keys and payment-method activation outside this workflow.

## Rollback

Disable this project's ingress flag first if the preview is unsafe. Restore the
previous reviewed image tags/revision and recreate only this Compose project's
web/API services; do not reset financial data or run reverse migrations. Keep the
database volume and private environment. `docker compose down` stops only this
project; never add `--volumes` during routine rollback. Keep the accepted source,
image IDs and redacted probe result. An empty first deployment can be stopped
without touching existing applications or their proxy routes.
