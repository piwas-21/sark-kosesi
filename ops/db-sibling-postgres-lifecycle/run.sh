#!/usr/bin/env bash
# Hosted, isolated synthetic lifecycle only. Never mount an existing volume.
set -euo pipefail
fail() { printf 'sibling lifecycle: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${LIFECYCLE_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$LIFECYCLE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$LIFECYCLE_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-sibling-lifecycle-evidence"
scratch="$RUNNER_TEMP/db-sibling-lifecycle-scratch"
mkdir "$out" "$scratch"
stage=composition
volume=''
control_volume=''
created_volumes=()
container=''
control=''
nonce=$(python3 -c 'import secrets; print(secrets.token_hex(16))')
owned_label="piwas.sibling-lifecycle=$nonce"
cleanup() {
  local rc=$?
  trap - EXIT
  for id in "$container" "$control"; do
    if [[ -n "$id" ]]; then
      if [[ "$(docker inspect --format '{{ index .Config.Labels "piwas.sibling-lifecycle" }}' "$id" 2>/dev/null || true)" == "$nonce" ]]; then
        docker rm -f "$id" >/dev/null || rc=1
      else
        rc=1
      fi
    fi
  done
  for name in "${created_volumes[@]}"; do
    if [[ "$(docker volume inspect --format '{{ index .Labels "piwas.sibling-lifecycle" }}' "$name" 2>/dev/null || true)" == "$nonce" ]]; then
      docker volume rm "$name" >/dev/null || rc=1
    else
      rc=1
    fi
  done
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
  exit "$rc"
}
trap cleanup EXIT
printf 'revision=%s\n' "$LIFECYCLE_REVISION" > "$out/identity.txt"
COMPOSE_REVISION="$LIFECYCLE_REVISION" bash ops/db-sibling-postgres/run.sh
grep -Fxq 'stage=complete outcome=success' "$RUNNER_TEMP/db-sibling-postgres-evidence/status.txt"
image=$(docker image inspect "local/db-sibling-postgres:$LIFECYCLE_REVISION" --format '{{.Id}}')
[[ "$image" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'invalid image identity'
grep -Fxq "composed=$image" "$RUNNER_TEMP/db-sibling-postgres-evidence/image-ids.txt"
printf 'image=%s\n' "$image" >> "$out/identity.txt"

stage=prepare_volumes
volume="piwas-sibling-lifecycle-$nonce"
control_volume="piwas-sibling-lifecycle-control-$nonce"
for name in "$volume" "$control_volume"; do
  if docker volume inspect "$name" >/dev/null 2>&1; then fail 'volume already exists'; fi
  [[ "$(docker volume create --label "$owned_label" "$name")" == "$name" ]] || fail 'volume creation failed'
  created_volumes+=("$name")
  [[ "$(docker volume inspect --format '{{ index .Labels "piwas.sibling-lifecycle" }}' "$name")" == "$nonce" ]] || fail 'volume label mismatch'
done
# Only these fresh named volumes are mounted; no recursive ownership change.
docker run --rm --network none --read-only --cap-drop ALL --cap-add CHOWN --security-opt no-new-privileges \
  --user 0:0 --mount "type=volume,src=$volume,dst=/owned,volume-nocopy" --entrypoint sh "$image" -ec \
  'test -z "$(ls -A /owned)"; mkdir -m 755 /owned/18; mkdir -m 700 /owned/18/docker; chown 999:999 /owned/18/docker'
docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
  --user 0:0 --mount "type=volume,src=$control_volume,dst=/owned,volume-nocopy" --entrypoint sh "$image" -ec \
  'test -z "$(ls -A /owned)"; mkdir -m 755 /owned/18; mkdir -m 700 /owned/18/docker'

create_server() {
  docker create --label "$owned_label" --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
    --user 999:999 --memory 512m --cpus 1 --pids-limit 128 --stop-signal SIGINT --stop-timeout 30 \
    --tmpfs /tmp:rw,nosuid,nodev,mode=1777,size=32m \
    --mount "type=volume,src=$1,dst=/var/lib/postgresql,volume-nocopy" \
    --env PGDATA=/var/lib/postgresql/18/docker --entrypoint /opt/pg-smoke/bin/postgres "$image" \
    -D /var/lib/postgresql/18/docker -c listen_addresses=127.0.0.1 \
    -c unix_socket_directories=/tmp -c shared_buffers=16MB
}
expect_start_failure() {
  local name=$1 pattern=$2
  control=$(create_server "$name")
  docker start "$control" >/dev/null
  for _ in {1..20}; do
    [[ "$(docker inspect --format '{{.State.Running}}' "$control")" == false ]] && break
    sleep 1
  done
  [[ "$(docker inspect --format '{{.State.Running}}' "$control")" == false ]] || fail 'control did not exit'
  [[ "$(docker inspect --format '{{.State.ExitCode}}' "$control")" != 0 ]] || fail 'control unexpectedly succeeded'
  docker logs "$control" > "$scratch/control.log" 2>&1
  grep -Fq "$pattern" "$scratch/control.log" || fail 'control failed for unexpected reason'
  docker rm "$control" >/dev/null
  control=''
}
stage=negative_controls
expect_start_failure "$volume" 'postgresql.conf'
expect_start_failure "$control_volume" 'Permission denied'

stage=initialize
# The password is synthetic and only exists in this container's tmpfs.
docker run --rm -i --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
  --user 999:999 --tmpfs /tmp:rw,nosuid,nodev,mode=1777,size=32m \
  --mount "type=volume,src=$volume,dst=/var/lib/postgresql,volume-nocopy" --entrypoint sh "$image" -s <<'INIT'
set -eu
umask 077
test "$(stat -c '%u:%g:%a' /var/lib/postgresql/18/docker)" = 999:999:700
test -z "$(ls -A /var/lib/postgresql/18/docker)"
printf '%s\n' 'synthetic-lifecycle-password' > /tmp/password
/opt/pg-smoke/bin/initdb -D /var/lib/postgresql/18/docker --locale=en_US.utf8 --encoding=UTF8 \
  --auth-local=scram-sha-256 --auth-host=scram-sha-256 --username=postgres --pwfile=/tmp/password >/dev/null
rm /tmp/password
INIT

wait_ready() {
  for _ in {1..30}; do
    if docker exec "$container" /opt/pg-smoke/bin/pg_isready -q -h 127.0.0.1 -U postgres -d postgres; then return; fi
    [[ "$(docker inspect --format '{{.State.Running}}' "$container")" == true ]] || fail 'server exited before readiness'
    sleep 1
  done
  fail 'server readiness timed out'
}
inspect_runtime() {
  docker inspect "$container" > "$scratch/container.json"
  python3 ops/db-sibling-postgres-lifecycle/validate.py "$scratch/container.json" "$image" "$volume" "$1"
}
query() {
  docker exec -e PGPASSWORD=synthetic-lifecycle-password "$container" /opt/pg-smoke/bin/psql \
    -XAt -v ON_ERROR_STOP=1 -h 127.0.0.1 -U postgres -d postgres -c "$1"
}
start_server() {
  container=$(create_server "$volume")
  docker start "$container" >/dev/null
  wait_ready
  inspect_runtime running
  [[ "$(docker exec "$container" cat /proc/1/comm)" == postgres ]] || fail 'postgres is not PID 1'
  docker exec "$container" cat /proc/1/status > "$scratch/pid1-status.txt"
  awk '$1 == "Uid:" { if ($2 != 999 || $3 != 999 || $4 != 999 || $5 != 999) exit 1; found=1 } END { if (!found) exit 1 }' "$scratch/pid1-status.txt" || fail 'incorrect process UID'
  awk '$1 == "Gid:" { if ($2 != 999 || $3 != 999 || $4 != 999 || $5 != 999) exit 1; found=1 } END { if (!found) exit 1 }' "$scratch/pid1-status.txt" || fail 'incorrect process GID'
  awk '$1 == "Groups:" { for (i=2; i<=NF; i++) if ($i == 0) exit 1; found=1 } END { if (!found) exit 1 }' "$scratch/pid1-status.txt" || fail 'unsafe supplementary groups'
}
stop_server() {
  docker stop -t 30 "$container" >/dev/null
  inspect_runtime stopped
  docker logs "$container" > "$scratch/server.log" 2>&1
  grep -Fq 'database system is shut down' "$scratch/server.log" || fail 'clean shutdown not observed'
  docker rm "$container" >/dev/null
  container=''
}
stage=first_start
start_server
[[ "$(query 'SELECT 42')" == 42 ]] || fail 'authenticated query failed'
if docker exec -e PGPASSWORD=wrong-synthetic-password "$container" /opt/pg-smoke/bin/psql \
  -XAt -v ON_ERROR_STOP=1 -h 127.0.0.1 -U postgres -d postgres -c 'SELECT 42' > "$scratch/auth.log" 2>&1; then
  fail 'wrong password accepted'
fi
grep -Fq 'password authentication failed' "$scratch/auth.log" || fail 'authentication control failed for another reason'
query "CREATE TABLE lifecycle_fixture (id integer PRIMARY KEY, value text NOT NULL); INSERT INTO lifecycle_fixture VALUES (1, 'durable-synthetic');" >/dev/null
stage=stop_and_recreate
stop_server
start_server
[[ "$(query 'SELECT id || chr(58) || value FROM lifecycle_fixture')" == '1:durable-synthetic' ]] || fail 'sentinel missing after recreation'
stop_server
printf 'uninitialized=pass wrong_owner=pass auth_positive=pass auth_negative=pass pid1=pass clean_stop=pass recreate_sentinel=pass\n' > "$out/results.txt"
