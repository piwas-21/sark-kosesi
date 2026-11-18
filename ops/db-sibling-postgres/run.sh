#!/usr/bin/env bash
# Disposable AMD64 composition; only synthetic data and isolated containers.
set -euo pipefail
fail() { printf 'sibling postgres: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${COMPOSE_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$COMPOSE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$COMPOSE_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-sibling-postgres-evidence"
scratch="$RUNNER_TEMP/db-sibling-postgres-scratch"
mkdir "$out" "$scratch" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
source_id=''
target_id=''
zlib_proof_id=''
stage=prerequisites
cleanup() {
  rc=$?
  for id in "$source_id" "$target_id" "$zlib_proof_id"; do
    if [[ -n "$id" ]]; then docker rm -f -v "$id" >/dev/null || true; fi
  done
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
}
trap cleanup EXIT
printf 'revision=%s\n' "$COMPOSE_REVISION" > "$out/revision.txt"
SIBLING_REVISION="$COMPOSE_REVISION" bash ops/db-sibling-runtime-root/run.sh
SMOKE_REVISION="$COMPOSE_REVISION" bash ops/db-source-feature-smoke/run.sh
grep -Fxq 'stage=complete outcome=success' "$RUNNER_TEMP/db-sibling-root-evidence/status.txt"
grep -Fxq 'stage=complete outcome=success' "$RUNNER_TEMP/db-source-feature-evidence/status.txt"
root_image="local/db-sibling-root:$COMPOSE_REVISION"
source_image="local/pg-source-feature:$COMPOSE_REVISION"
root_id=$(docker image inspect "$root_image" --format '{{.Id}}')
source_image_id=$(docker image inspect "$source_image" --format '{{.Id}}')
grep -Fxq "candidate=$root_id" "$RUNNER_TEMP/db-sibling-root-evidence/image-ids.txt"
grep -Fxq "candidate_image_id=$source_image_id" "$RUNNER_TEMP/db-source-feature-evidence/identity.txt"
stage=zlib_package
mkdir "$scratch/zlib-context"
cp ops/db-zlib-candidate/{Dockerfile,sanitize_apt_failure.sh} "$scratch/zlib-context/"
cp /etc/ssl/certs/ca-certificates.crt "$scratch/zlib-context/ca-certificates.crt"
test -s "$scratch/zlib-context/ca-certificates.crt"
zlib_image="local/db-sibling-zlib-source:$COMPOSE_REVISION"
docker build --platform linux/amd64 -t "$zlib_image" -f "$scratch/zlib-context/Dockerfile" "$scratch/zlib-context" > "$out/zlib-build.log" 2>&1
zlib_id=$(docker image inspect "$zlib_image" --format '{{.Id}}')
[[ "$zlib_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'invalid verified package image'
docker image inspect "$zlib_id" > "$out/zlib-image.json"
docker image inspect "mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452" > "$out/zlib-stock-image.json"
zlib_proof_id=$(docker create --network none "$zlib_id")
docker cp "$zlib_proof_id:/usr/local/share/db-zlib-candidate-evidence" "$out/zlib-package-evidence"
docker rm -v "$zlib_proof_id" >/dev/null
zlib_proof_id=''
stage=build
mkdir "$scratch/build-context"
cp ops/db-sibling-postgres/{Dockerfile,packages.lock} "$scratch/build-context/"
cp /etc/ssl/certs/ca-certificates.crt "$scratch/build-context/ca-certificates.crt"
test -s "$scratch/build-context/ca-certificates.crt"
image="local/db-sibling-postgres:$COMPOSE_REVISION"
docker build --platform linux/amd64 --build-arg "SOURCE_IMAGE=$source_image" --build-arg "SIBLING_IMAGE=$root_image" --build-arg "ZLIB_IMAGE=$zlib_image" -t "$image" -f "$scratch/build-context/Dockerfile" "$scratch/build-context" > "$out/build.log" 2>&1
candidate_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$root_id" && "$candidate_id" != "$source_image_id" && "$candidate_id" != "$zlib_id" ]] || fail 'invalid composition identity'
printf 'sibling=%s\nsource=%s\nzlib_source=%s\ncomposed=%s\n' "$root_id" "$source_image_id" "$zlib_id" "$candidate_id" > "$out/image-ids.txt"
docker image inspect "$root_id" > "$out/sibling-image.json"
docker image inspect "$candidate_id" > "$out/composed-image.json"
cp "$RUNNER_TEMP/db-sibling-root-evidence/candidate-packages.txt" "$out/sibling-packages.txt"
stage=inventory
for profile in source composed; do
  if [[ "$profile" == source ]]; then current=$source_image_id; else current=$candidate_id; fi
  for probe in files metadata ownership; do
    docker run --rm -i --network none --entrypoint bash "$current" -s -- "$probe" < ops/db-sibling-postgres/inspect.sh > "$out/$profile-$probe.txt"
  done
done
docker run --rm -i --network none --entrypoint sh "$candidate_id" -s -- packages < ops/db-sibling-runtime-root/probe.sh > "$out/composed-packages.txt"
docker run --rm -i --network none --entrypoint bash "$candidate_id" -s -- linkage < ops/db-sibling-postgres/inspect.sh > "$out/composed-linkage.txt"
docker run --rm --network none --entrypoint cat "$candidate_id" /usr/local/share/sibling-postgres-evidence/trust-inputs.txt > "$out/trust-inputs.txt"
for point in before after; do
  docker run --rm --network none --entrypoint cat "$candidate_id" "/usr/local/share/sibling-postgres-evidence/zlib-$point.txt" > "$out/zlib-$point.txt"
done
docker run --rm --network none --user postgres --entrypoint bash "$candidate_id" -c 'set -eu; test "$(id -u)" = 999; test "$(id -g)" = 999; test ! -w /opt/pg-smoke/bin/postgres'
stage=fixture_start
stock='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
source_id=$(docker run -d --network none --user postgres --tmpfs /tmp:rw,size=256m --memory 768m --cpus 1 --pids-limit 256 --entrypoint sleep "$stock" 1800)
target_id=$(docker run -d --network none --user postgres --tmpfs /tmp:rw,size=256m --memory 768m --cpus 1 --pids-limit 256 --entrypoint sleep "$candidate_id" 1800)
# shellcheck source=ops/db-source-feature-smoke/transfer.sh
source ops/db-source-feature-smoke/transfer.sh
stage=helper_transfer
transfer_in "$source_id" /tmp/diagnostics.sh ops/db-source-feature-smoke/diagnostics.sh 16384
transfer_in "$target_id" /tmp/diagnostics.sh ops/db-source-feature-smoke/diagnostics.sh 16384
stage=source_fixture
docker exec -i "$source_id" bash -s -- /usr/lib/postgresql/18/bin source < ops/db-source-feature-smoke/fixture.sh > "$out/source-fixture.txt"
stage=archive_transfer
transfer_out "$source_id" /tmp/feature-smoke.dump "$scratch/feature-smoke.dump" 1048576
transfer_in "$target_id" /tmp/feature-smoke.dump "$scratch/feature-smoke.dump" 1048576
sha256sum "$scratch/feature-smoke.dump" | cut -d ' ' -f 1 > "$out/archive-sha256.txt"
stage=target_fixture
docker exec -i "$target_id" bash -s -- /opt/pg-smoke/bin target < ops/db-source-feature-smoke/fixture.sh > "$out/target-fixture.txt"
stage=loaded_libraries
docker exec -i "$target_id" bash -s < ops/db-sibling-postgres/loaded.sh > "$out/loaded-libraries.txt"
stage=validate
python3 ops/db-sibling-postgres/validate.py "$out" > "$out/validation.txt"
