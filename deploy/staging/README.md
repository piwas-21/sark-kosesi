# Non-selling staging preview

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
published only on host loopback. The base stack routes only web ingress. The
optional callback edge below has a separate disabled-by-default router. The
default remains a private preview. Set
`STAGING_PUBLIC_STOREFRONT=true` to open the existing storefront GET/HEAD routes
without a browser login. Keep noindex/no-store, disabled production and internal
API authentication. Unknown routes and mutations remain gated.
No expiry worker, admin authority, merchant data or test fixtures are seeded.

## Build and start

The web build copies each locked workspace manifest before `npm ci` so that
workspace dependency changes invalidate the install layer. Keep those copies in
sync when adding or changing workspaces.

1. Use accepted `develop` source whose complete tree matches the independently
   reviewed feature and all nine current-head gates. Keep `productionEnabled=false`.
   Before upgrading an existing database, prove migration/startup compatibility
   and the recovery path described under [Rollback](#rollback).
2. Resolve the staging hostname to the intended server and verify the existing
   proxy/network before creating any resources. Do not restart or reconfigure
   existing applications. Set the hostname and exact accepted commit in a private
   environment file; generate independent passwords/tokens and leave ingress false.
   Set `STAGING_WEB_LOOPBACK_PORT` to a distinct unused host port in a candidate
   project's private file when the current preview must stay running. The
   default is 45972. The host binding stays on 127.0.0.1, and the container and
   Traefik service still use port 3000. Check existing container mappings and
   host listeners before starting either project.
3. Build immutable local image tags from that exact source, sequentially. Rust
   job and Node heap settings alone do not bound a shared host. Verify the actual
   driver's kernel memory/CPU/swap/process limits, including a `RUN` descendant
   under the capped parent cgroup, and measure host memory/disk reserve. Recheck
   limits after builder restarts. Use the explicit local daemon, an owned builder
   and private Docker configuration; preserve
   other applications and the default builder. Retain build results and image IDs.
   Export each archive, stop that builder, then import sequentially with reserve
   monitoring: daemon image import is outside the builder's cap. The examples
   assume those builder/resource checks have passed.

```sh
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx build --builder OWNED_BUILDER --file deploy/staging/Dockerfile --target api --build-arg SOURCE_REVISION=REVISION --tag sark-kosesi-api:REVISION --output type=docker,dest=PRIVATE_API_ARCHIVE .
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx stop OWNED_BUILDER
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock load --input PRIVATE_API_ARCHIVE
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx build --builder OWNED_BUILDER --file deploy/staging/Dockerfile --target web --build-arg SOURCE_REVISION=REVISION --tag sark-kosesi-web:REVISION --output type=docker,dest=PRIVATE_WEB_ARCHIVE .
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx stop OWNED_BUILDER
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock load --input PRIVATE_WEB_ARCHIVE
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock compose --project-name OWNED_PROJECT --env-file PRIVATE_ENV --file deploy/staging/compose.yml config --quiet
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock compose --project-name OWNED_PROJECT --env-file PRIVATE_ENV --file deploy/staging/compose.yml up --detach --wait
```

4. With ingress disabled, verify both images/processes and the dedicated database.
   The API applies the committed forward migrations. The web health check requires
   the configured storefront access mode, an anonymous admin 401 challenge and API
   readiness, without disclosing credentials.
5. After the exact-source checks and private startup pass, enable only this
   project's ingress flag and recreate its web/API services. The host rule must be
   the chosen staging hostname and the TLS resolver must be `letsencrypt`. Verify a
   valid certificate without disabling TLS verification. No public database/API port
   or existing router may be added/changed.
6. Store the applicable credential by itself in a private local file and run the
   read-only acceptance check. It verifies the chosen storefront access mode,
   protected admin shell, exact served source, redirect origin, noindex/no-store,
   RTL and disabled commerce. For public storefront mode generate a separate
   random `STAGING_ADMIN_TOKEN` (32–128 URL-safe characters, distinct from the
   internal preview token). Browser username is `test-admin`. This is a Basic-auth
   gate to the unavailable operations shell, not a staff session or permission
   grant. Real login/MFA/session issuance and HTTP admin operations remain P4.
   Never seed a privileged database account just to make this shell accessible.

```sh
node tests/application/staging-smoke.mjs --origin https://STAGING_HOST --revision REVISION --token-file PRIVATE_TOKEN_FILE
# Public storefront mode; use a private file containing only the admin token:
node tests/application/staging-smoke.mjs --origin https://STAGING_HOST --revision REVISION --public-storefront --admin-token-file PRIVATE_ADMIN_TOKEN_FILE
```

This is preview acceptance, not actual order/payment/load/fulfilment or production
acceptance. Use only a dedicated test profile/key for later Mollie work. Keep live
keys and payment-method activation outside this workflow.

## Optional callback edge — source only, ingress disabled

The base file does not start a callback edge or add an anonymous POST exception.
The separate `callback-edge.compose.yml` overlay uses JSON-compatible Compose
syntax so the source policy check needs no additional parser dependency. Include
both files and explicitly select the `callback-edge` profile only after the edge
source is accepted through independent review and all nine current-head checks.
Build the third image from that same accepted tree with the builder/resource and
archive-import checks above:

```sh
python3 scripts/check_callback_edge_deploy.py
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx build --builder OWNED_BUILDER --file deploy/staging/Dockerfile --target callback-edge --build-arg SOURCE_REVISION=REVISION --tag sark-kosesi-callback-edge:REVISION --output type=docker,dest=PRIVATE_EDGE_ARCHIVE .
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock buildx stop OWNED_BUILDER
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock load --input PRIVATE_EDGE_ARCHIVE
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock compose --project-name OWNED_PROJECT --env-file PRIVATE_ENV --file deploy/staging/compose.yml --file deploy/staging/callback-edge.compose.yml --profile callback-edge config --quiet
```

Keep `STAGING_CALLBACK_INGRESS_ENABLED=false`. The overlay requires an explicitly
configured dedicated Test `CALLBACK_INTAKE_PROFILE_ID` for the private API and one
to four inspected exact private `CALLBACK_EDGE_PROXY_IPS` for the edge. Do not
substitute a Coolify subnet, a shared SNAT gateway, a hostname or a guessed address.
The process uses the existing project's server Bearer credential, shares web's
network namespace and forwards only to `127.0.0.1:45671`. It receives no database
credentials or Mollie key. Its port 45672 has no host mapping. The health check is
only a bounded process-liveness GET; it does not prove API, database or provider
acceptance. Preserve the private environment file's mode 0600; never print a full
rendered Compose document from that file.

With those prerequisites prepared, private startup uses the same two files and
profile:

```sh
docker --config PRIVATE_DOCKER_CONFIG --host unix:///var/run/docker.sock compose --project-name OWNED_PROJECT --env-file PRIVATE_ENV --file deploy/staging/compose.yml --file deploy/staging/callback-edge.compose.yml --profile callback-edge up --detach --wait
```

The edge container owns its Traefik labels. When it is absent or its independent
ingress flag is false, it contributes no anonymous router. Existing web labels,
Next's Basic gate and public storefront GET/HEAD policy remain unchanged. The
optional router uses only entrypoint `https`, the exact staging host and
`POST /api/callbacks/mollie`, with an explicit service and priority. There is no
HTTP callback redirect router. Configure provider URLs as canonical HTTPS URLs.

The middleware chain applies per-remote-address rate limits, overwrites a
nonsensitive constant budget header, applies a global 16-request inflight group,
and buffers at most 1024 bytes in memory. It has no retry expression. That header
is a resource-group identifier, never an authentication credential. Rust verifies
the actual socket peer, ignores forwarding headers for trust, imposes its own
concurrency/body/deadline limits, strips caller headers and acknowledges only a
finished empty 200 from the private API.

These files do not establish the running proxy's identity or behavior. Before any
public activation, separately record and verify:

- The actual proxy image/version and its support for container-shared network
  address discovery. The reviewed [Traefik v3.3 Docker provider source](https://raw.githubusercontent.com/traefik/traefik/v3.3.0/pkg/provider/docker/config.go)
  supports this topology; the installed version remains an external dependency.
- A unique, stable mapping from the edge's observed socket peers to that proxy,
  and denial of direct network/host access, including forged forwarding headers.
  This is a network ACL, not cryptographic proxy identity or Mollie authentication.
- Actual header replacement, rate/inflight/buffering behavior, absence of inherited
  POST retries and nonzero ingress read timeouts. [Buffering](https://doc.traefik.io/traefik/v3.3/middlewares/http/buffering/)
  reads the client body before forwarding, so Rust's one-second body deadline
  cannot bound an upload still held by Traefik. The complete upload/processing
  budget must fit the provider's delivery deadline. Do not reconfigure or restart
  the shared proxy implicitly to satisfy this gate.
- Exact-revision TLS, protected admin/unknown paths, private API denial and actual
  provider delivery/retry behavior in an authorized isolated non-selling pilot.
  Never run that pilot against a shared development or production database.

Only after that evidence and the applicable explicit activation authorization may
this project's callback ingress flag become true. Source/config checks and the
existing read-only staging smoke cannot prove callback acceptance. No worker,
provider key, scheduler, financial/stock action or production activation is added
by the overlay. The fixed limits are engineering policy, not measured production
capacity.

## Rollback

Disable `STAGING_CALLBACK_INGRESS_ENABLED` first if the optional callback edge is
unsafe, and disable `STAGING_INGRESS_ENABLED` when stopping all preview ingress.
Recreate only this project's affected services with the same overlay/profile
selection; do not change another application's router. Before a schema upgrade,
prove actual startup against stored migration versions/checksums and each image's
embedded manifest. SQLx can reject newer recorded migrations absent from an older
image, even when the old read queries remain compatible. Restore previous images
against that database only after this proof; never delete migration history,
reverse migrations or reset data to make rollback work.

For a quiescent non-selling preview, preserve the old database, volume and
containers and restore a consistent private backup into a new owned Compose
project/database. Before starting its database, check exact rendered volume and
network names, ownership labels, loopback port and fixed Traefik router/service
name collisions. Use the candidate's distinct loopback port for private host
probes and keep the original project's port assignment for rollback.
Keep candidate ingress disabled; verify the actual isolated
backend/volume identity before restoring. Compare restored per-table counts and
content hashes with the same consistent snapshot; record archive hashes and
migration checksums. Apply forward migrations only in the clone.
Verify accepted image identities, private health/admin/privacy guards and disabled
features before cutover.

Before switching, stop the exact original API and web containers in that order.
Keep the source database on its isolated backend. Verify the source has no other
database clients or prepared transactions while capturing the final snapshot,
and compare the table contents, migration history and sequence state with the
clone's original source snapshot.

Verify that Traefik has removed both preview routers before starting the
candidate. Docker may retain an external bridge endpoint for an exited
container. Check actual router state as well as exact container state; Docker
network records alone do not establish which backend Traefik serves. Start the
candidate web before its API because they share the web network namespace.
Verify private API readiness, the only active preview backend, trusted HTTPS,
served revision and storefront/admin access from an external client.

For rollback, stop the exact candidate API and web and verify its routes are
absent. Retain both databases and compare their data with the verified
snapshots before moving traffic. If either database has business writes after
its snapshot, require reviewed catch-up and reconciliation before moving
traffic either way. Start the preserved original web and API against their
original database, then repeat the namespace, route and HTTPS checks. Keep
this preview evidence separate from the production migration and recovery plan.

Keep database volumes and private environments. `docker compose down` stops only
the explicitly selected project; never add `--volumes` during routine rollback.
Keep accepted source, image IDs and redacted probe results. An empty first
deployment can be stopped without touching other applications or proxy routes.
