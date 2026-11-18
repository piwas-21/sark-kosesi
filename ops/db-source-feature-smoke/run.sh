#!/usr/bin/env bash
# Hosted AMD64 runner only. All database inputs are synthetic.
set -euo pipefail
fail() { printf 'source feature smoke: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${SMOKE_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$SMOKE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$SMOKE_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-source-feature-evidence"
scratch="$RUNNER_TEMP/db-source-feature-scratch"
mkdir "$out" "$scratch"
mkdir "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
source_id=''
target_id=''
stage=identity
cleanup() {
  rc=$?
  for id in "$source_id" "$target_id"; do
    if [[ -n "$id" ]]; then docker rm -f -v "$id" >/dev/null || true; fi
  done
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
}
trap cleanup EXIT
base='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
base_id=$(sh ops/db-zlib-candidate/pull_verified_base.sh "$scratch/base-pull.log")
[[ "$base_id" == sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d ]] || fail 'stock identity mismatch'
printf 'revision=%s\nstock_manifest=%s\nstock_image_id=%s\nsource_sha256=%s\n' "$SMOKE_REVISION" "$base" "$base_id" '441dfae4280cae3e0425e5f876f80559fb0731c140f1aae22b3e5a54d0411bfd' > "$out/identity.txt"
stage=build
mkdir "$scratch/build-context"
cp ops/db-source-feature-smoke/Dockerfile ops/db-source-feature-smoke/packages.lock "$scratch/build-context/"
cp /etc/ssl/certs/ca-certificates.crt "$scratch/build-context/ca-certificates.crt"
test -s "$scratch/build-context/ca-certificates.crt"
docker build --platform linux/amd64 -t "local/pg-source-feature:$SMOKE_REVISION" -f "$scratch/build-context/Dockerfile" "$scratch/build-context" > "$out/build.log" 2>&1
candidate_id=$(docker image inspect "local/pg-source-feature:$SMOKE_REVISION" --format '{{.Id}}')
printf 'candidate_image_id=%s\n' "$candidate_id" >> "$out/identity.txt"
stage=inventory
for profile in stock candidate; do
  if [[ "$profile" == stock ]]; then image=$base_id; bin=/usr/lib/postgresql/18/bin; lib=/usr/lib/x86_64-linux-gnu/libpq.so.5; else image=$candidate_id; bin=/opt/pg-smoke/bin; lib=/opt/pg-smoke/lib/libpq.so; fi
  docker run --rm --network none --entrypoint /bin/bash "$image" -c 'set -eu; uname -m; dpkg --print-architecture; ldd --version | head -n 1; locale -a; "$1/pg_config" --version; "$1/pg_config" --configure; dpkg-query -W | LC_ALL=C sort' _ "$bin" > "$out/$profile-inventory.txt"
  docker run --rm --network none --entrypoint /bin/bash "$image" -c 'set -eu; ldd "$1/postgres"; ldd "$2"' _ "$bin" "$lib" > "$out/$profile-linkage.txt"
done
grep -Eq 'libxml2[.]' "$out/stock-linkage.txt" || fail 'stock XML positive control missing'
grep -Eq 'libgssapi[^ ]*[.]' "$out/stock-linkage.txt" || fail 'stock GSS positive control missing'
bash ops/db-source-feature-smoke/linkage.sh < "$out/candidate-linkage.txt" || fail 'candidate linkage assertion failed'
docker run --rm --network none --entrypoint cat "$candidate_id" /smoke-build-evidence/build-packages.txt > "$out/build-packages.txt"
docker run --rm --network none --entrypoint cat "$candidate_id" /smoke-build-evidence/trust-inputs.txt > "$out/trust-inputs.txt"
stage=fixture
source_id=$(docker run -d --network none --user postgres --tmpfs /tmp:rw,size=256m --memory 768m --cpus 1 --pids-limit 256 --entrypoint sleep "$base_id" 1800)
target_id=$(docker run -d --network none --user postgres --tmpfs /tmp:rw,size=256m --memory 768m --cpus 1 --pids-limit 256 --entrypoint sleep "$candidate_id" 1800)
docker cp ops/db-source-feature-smoke/diagnostics.sh "$source_id:/tmp/diagnostics.sh"
docker cp ops/db-source-feature-smoke/diagnostics.sh "$target_id:/tmp/diagnostics.sh"
docker exec -i "$source_id" bash -s -- /usr/lib/postgresql/18/bin source < ops/db-source-feature-smoke/fixture.sh > "$out/source-fixture.txt"
docker cp "$source_id:/tmp/feature-smoke.dump" "$scratch/feature-smoke.dump"
docker cp "$scratch/feature-smoke.dump" "$target_id:/tmp/feature-smoke.dump"
docker exec -i "$target_id" bash -s -- /opt/pg-smoke/bin target < ops/db-source-feature-smoke/fixture.sh > "$out/target-fixture.txt"
