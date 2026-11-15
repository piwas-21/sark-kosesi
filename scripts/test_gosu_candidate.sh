#!/usr/bin/env bash
# Synthetic, disposable hosted test. Never runs against a shared or deployed DB.
set -euo pipefail
fail() { printf 'gosu candidate: %s\n' "$1" >&2; exit 1; }
: "${GOSU_EVIDENCE_DIR:?empty evidence directory required}"
: "${GOSU_SCRATCH_DIR:?empty scratch directory required}"
: "${GOSU_SOURCE_REVISION:?reviewed source revision required}"
: "${RUNNER_TEMP:?hosted runner temp required}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'GitHub-hosted runner required'
[[ "$GOSU_EVIDENCE_DIR" == "$RUNNER_TEMP/db-gosu-evidence" && "$GOSU_SCRATCH_DIR" == "$RUNNER_TEMP/db-gosu-scratch" ]] || fail 'unexpected output path'
[[ "$GOSU_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$GOSU_SOURCE_REVISION" ]] || fail 'source revision mismatch'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux AMD64 required'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
mkdir -p "$GOSU_EVIDENCE_DIR" "$GOSU_SCRATCH_DIR"
[[ -z "$(ls -A "$GOSU_EVIDENCE_DIR")" && -z "$(ls -A "$GOSU_SCRATCH_DIR")" ]] || fail 'output directories must be empty'
evidence=$(cd "$GOSU_EVIDENCE_DIR" && pwd -P)
scratch=$(cd "$GOSU_SCRATCH_DIR" && pwd -P)
[[ "$evidence" != "$scratch" && "$scratch" != "$evidence"/* && "$evidence" != "$scratch"/* ]] || fail 'overlapping output directories'
source_commit=6456aaa0f3c854d199d0f037f068eb97515b7513
source_sha=33d7537d588ea49458b9509bcf4554bdf5ceacc66da71e5caa1058ea3b689c3b
go_sha=7226736c2339719cda28fbf3a54d71aa63bcefe376e98cef00034268dab24840
base='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
base_config='sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d'
image="local/db-gosu-candidate:${GOSU_SOURCE_REVISION}"
db_id=''
cleanup() {
  if [[ -n "$db_id" ]]; then docker rm -f -v "$db_id" >/dev/null || true; fi
}
trap cleanup EXIT
printf 'stage=download outcome=running\n' > "$evidence/status.txt"
printf 'source_revision=%s\nupstream_commit=%s\nbase=%s\n' "$GOSU_SOURCE_REVISION" "$source_commit" "$base" > "$evidence/identity.txt"
curl -qfsSL --retry 2 --max-time 120 --max-filesize 2000000 "https://codeload.github.com/tianon/gosu/tar.gz/$source_commit" -o "$scratch/source.tar.gz"
curl -qfsSL --retry 2 --max-time 240 --max-filesize 80000000 'https://go.dev/dl/go1.27.4.linux-amd64.tar.gz' -o "$scratch/go.tar.gz"
printf '%s  %s\n' "$source_sha" "$scratch/source.tar.gz" | sha256sum -c -
printf '%s  %s\n' "$go_sha" "$scratch/go.tar.gz" | sha256sum -c -
mkdir "$scratch/source" "$scratch/toolchain" "$scratch/mod" "$scratch/cache" "$scratch/gopath" "$scratch/tmp" "$scratch/one" "$scratch/two" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
tar -xzf "$scratch/source.tar.gz" --strip-components=1 -C "$scratch/source"
tar -xzf "$scratch/go.tar.gz" --strip-components=1 -C "$scratch/toolchain"
[[ "$("$scratch/toolchain/bin/go" version)" == 'go version go1.27.4 linux/amd64' ]] || fail 'Go toolchain version mismatch'
sha256sum "$scratch/source/go.mod" "$scratch/source/go.sum" > "$evidence/go-input-hashes.txt"
printf 'source_archive_sha256=%s\ngo_archive_sha256=%s\n' "$source_sha" "$go_sha" >> "$evidence/identity.txt"
printf 'stage=vendor outcome=running\n' > "$evidence/status.txt"
(
  cd "$scratch/source"
  env -i PATH=/usr/bin:/bin GOCACHE="$scratch/cache" GOMODCACHE="$scratch/mod" GOPATH="$scratch/gopath" GOTMPDIR="$scratch/tmp" \
    GOPROXY=https://proxy.golang.org GOSUMDB=off GOTOOLCHAIN=local GOENV=off GOWORK=off GOTELEMETRY=off \
    "$scratch/toolchain/bin/go" mod download
  env -i PATH=/usr/bin:/bin GOCACHE="$scratch/cache" GOMODCACHE="$scratch/mod" GOPATH="$scratch/gopath" GOTMPDIR="$scratch/tmp" \
    GOPROXY=https://proxy.golang.org GOSUMDB=off GOTOOLCHAIN=local GOENV=off GOWORK=off GOTELEMETRY=off \
    "$scratch/toolchain/bin/go" mod vendor
)
sha256sum -c "$evidence/go-input-hashes.txt" >/dev/null || fail 'upstream go.mod or go.sum changed'
grep -Fxq '# github.com/moby/sys/user v0.1.0' "$scratch/source/vendor/modules.txt" || fail 'missing locked user dependency'
grep -Fxq '# golang.org/x/sys v0.1.0' "$scratch/source/vendor/modules.txt" || fail 'missing locked system dependency'
# This hash records the prepared input, not independent provenance for upstream source.
(cd "$scratch/source" && find vendor -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) > "$evidence/vendor-sha256.txt"
[[ -s "$evidence/vendor-sha256.txt" ]] || fail 'empty vendor inputs'
docker pull --platform linux/amd64 "$base" >/dev/null
[[ "$(docker image inspect "$base" --format '{{.Id}}')" == "$base_config" ]] || fail 'base config mismatch'
printf 'stage=offline_build outcome=running\n' > "$evidence/status.txt"
for output in one two; do
  docker run --rm --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
    --user "$(id -u):$(id -g)" --memory 2g --cpus 2 --pids-limit 128 \
    --tmpfs /tmp:rw,nosuid,nodev,mode=1777,size=1024m \
    --mount "type=bind,src=$scratch/source,dst=/src,readonly" \
    --mount "type=bind,src=$scratch/toolchain,dst=/toolchain,readonly" \
    --mount "type=bind,src=$scratch/$output,dst=/out" --workdir /src \
    --env GOCACHE=/tmp/cache --env GOPATH=/tmp/gopath --env GOTMPDIR=/tmp --env GOPROXY=off --env GOSUMDB=off \
    --env GOTOOLCHAIN=local --env GOENV=off --env GOWORK=off --env GOTELEMETRY=off \
    --env GOOS=linux --env GOARCH=amd64 --env GOAMD64=v1 --env CGO_ENABLED=0 \
    --entrypoint /toolchain/bin/go "$base" build -trimpath -buildvcs=false -mod=vendor -ldflags=-buildid= -o /out/gosu .
done
cmp "$scratch/one/gosu" "$scratch/two/gosu" || fail 'independent build bytes differ'
(cd "$scratch/source" && sha256sum -c "$evidence/vendor-sha256.txt" >/dev/null) || fail 'vendored inputs changed'
sha256sum "$scratch/one/gosu" | awk '{print $1}' > "$evidence/gosu-sha256.txt"
"$scratch/toolchain/bin/go" version -m "$scratch/one/gosu" > "$evidence/go-version-m.txt"
grep -Fq ': go1.27.4' "$evidence/go-version-m.txt" || fail 'unexpected compiled Go version'
grep -Fq $'\tpath\tgithub.com/tianon/gosu' "$evidence/go-version-m.txt" || fail 'unexpected Go module'
for flag in GOOS=linux GOARCH=amd64 GOAMD64=v1 CGO_ENABLED=0; do
  grep -Fxq $'\tbuild\t'"$flag" "$evidence/go-version-m.txt" || fail 'unexpected compiled build configuration'
done
cp "$scratch/one/gosu" "$scratch/context-gosu"
mkdir "$scratch/image-context"
cp "$scratch/context-gosu" "$scratch/image-context/gosu"
cp ops/db-gosu-candidate/Dockerfile "$scratch/image-context/Dockerfile"
printf 'stage=synthetic_candidate outcome=running\n' > "$evidence/status.txt"
docker build --network none --platform linux/amd64 --tag "$image" --file "$scratch/image-context/Dockerfile" "$scratch/image-context" > "$evidence/image-build.log" 2>&1
candidate_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$base_config" ]] || fail 'candidate image identity'
printf 'candidate_image_id=%s\n' "$candidate_id" >> "$evidence/identity.txt"
installed_sha=$(docker run --rm --network none --entrypoint sha256sum "$image" /usr/local/bin/gosu | awk '{print $1}')
[[ "$installed_sha" == "$(cat "$evidence/gosu-sha256.txt")" ]] || fail 'candidate executable identity mismatch'
selected_path=$(docker run --rm --network none --entrypoint sh "$image" -c 'command -v gosu')
[[ "$selected_path" == /usr/local/bin/gosu ]] || fail 'unexpected entrypoint PATH selection'
# The pinned base, not a guessed singleton, defines PostgreSQL's supplementary groups.
normalize_groups() {
  local value=$1
  [[ "$value" =~ ^[0-9]+(\ [0-9]+)*$ ]] || return 1
  printf '%s\n' "$value" | tr ' ' '\n' | LC_ALL=C sort -n -u | paste -sd ',' -
}
uid=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -u)
gid=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -g)
groups=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -G)
base_groups=$(docker run --rm --network none --entrypoint /usr/bin/id "$base" -G postgres)
[[ "$uid" =~ ^[0-9]+$ && "$gid" =~ ^[0-9]+$ ]] || fail 'invalid postgres numeric identity'
group_set=$(normalize_groups "$groups") || fail 'invalid candidate group list'
base_group_set=$(normalize_groups "$base_groups") || fail 'invalid base group list'
printf 'postgres_uid=%s\npostgres_gid=%s\ncandidate_group_set=%s\nbase_group_set=%s\n' \
  "$uid" "$gid" "$group_set" "$base_group_set" > "$evidence/identity-check.txt"
[[ "$uid" == 999 && "$gid" == 999 && "$group_set" == "$base_group_set" ]] || fail 'incorrect postgres identity'
[[ ",$group_set," != *,0,* && ",$group_set," == *,999,* ]] || fail 'unsafe postgres groups'
if docker run --rm --network none --user 999:999 --entrypoint /usr/local/bin/gosu "$image" root id > "$scratch/denied.log" 2>&1; then
  fail 'nonroot privilege escalation unexpectedly succeeded'
fi
db_id=$(docker run --detach --network none --memory 768m --cpus 1 --pids-limit 256 \
  --tmpfs /var/lib/postgresql:rw,size=256m --env POSTGRES_PASSWORD=isolated-synthetic-password "$image")
for ((attempt=0; attempt<60; attempt++)); do
  if docker exec "$db_id" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then break; fi
  [[ "$attempt" -lt 59 ]] || fail 'synthetic database startup timed out'
  sleep 1
done
result=$(docker exec "$db_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d postgres -c 'SELECT 2+3')
[[ "$result" == 5 ]] || fail 'synthetic database query mismatch'
printf 'postgres_uid=%s\npostgres_gid=%s\npostgres_groups=%s\nnonroot_to_root=denied\nsynthetic_query=%s\n' "$uid" "$gid" "$group_set" "$result" > "$evidence/synthetic-checks.txt"
printf 'stage=complete outcome=success\n' > "$evidence/status.txt"
(cd "$evidence" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
mv "$scratch/SHA256SUMS" "$evidence/SHA256SUMS"
