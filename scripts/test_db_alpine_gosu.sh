#!/usr/bin/env bash
# Disposable, hosted comparison. No real data, ingress, or deployment.
set -Eeuo pipefail
fail() { printf 'alpine gosu lab: %s\n' "$1" >&2; exit 1; }
: "${ALPINE_GOSU_SOURCE_REVISION:?source revision required}"
: "${ALPINE_GOSU_EVIDENCE_DIR:?evidence directory required}"
: "${ALPINE_GOSU_SCRATCH_DIR:?scratch directory required}"
: "${RUNNER_TEMP:?hosted runner temp required}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'GitHub-hosted runner required'
[[ "$ALPINE_GOSU_EVIDENCE_DIR" == "$RUNNER_TEMP/db-alpine-gosu-evidence" && "$ALPINE_GOSU_SCRATCH_DIR" == "$RUNNER_TEMP/db-alpine-gosu-scratch" ]] || fail 'unexpected output path'
[[ "$ALPINE_GOSU_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$ALPINE_GOSU_SOURCE_REVISION" ]] || fail 'source revision mismatch'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux AMD64 required'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
mkdir -p "$ALPINE_GOSU_EVIDENCE_DIR" "$ALPINE_GOSU_SCRATCH_DIR"
[[ -z "$(ls -A "$ALPINE_GOSU_EVIDENCE_DIR")" && -z "$(ls -A "$ALPINE_GOSU_SCRATCH_DIR")" ]] || fail 'output directories must be empty'
evidence=$(cd "$ALPINE_GOSU_EVIDENCE_DIR" && pwd -P)
scratch=$(cd "$ALPINE_GOSU_SCRATCH_DIR" && pwd -P)
[[ "$evidence" != "$scratch" && "$scratch" != "$evidence"/* && "$evidence" != "$scratch"/* ]] || fail 'overlapping output directories'
mkdir "$scratch/docker-config" "$scratch/context"
export DOCKER_CONFIG="$scratch/docker-config"
stock='mirror.gcr.io/library/postgres@sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366'
stock_config='sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e'
scanner='mirror.gcr.io/aquasec/trivy@sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c'
expected_gosu=8177d9590273ed8699065bf0c9776b634fde24bfc75375c83e927134ce868b7a
image="local/db-alpine-gosu-candidate:${ALPINE_GOSU_SOURCE_REVISION}"
db_id=''
stage=gosu_custody
cleanup() {
  local rc=$?
  if [[ "$rc" -ne 0 ]]; then printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$evidence/status.txt"; fi
  if [[ -n "$db_id" ]]; then docker rm -f -v "$db_id" >/dev/null || true; fi
}
trap cleanup EXIT
printf 'source_revision=%s\nstock=%s\nscanner=%s\n' "$ALPINE_GOSU_SOURCE_REVISION" "$stock" "$scanner" > "$evidence/identity.txt"
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
gosu_evidence="$RUNNER_TEMP/db-gosu-evidence"
gosu_scratch="$RUNNER_TEMP/db-gosu-scratch"
[[ "$(cat "$gosu_evidence/status.txt")" == 'stage=complete outcome=success' ]] || fail 'prior Gosu build incomplete'
(cd "$gosu_evidence" && sha256sum -c SHA256SUMS >/dev/null) || fail 'prior Gosu evidence checksum mismatch'
grep -Fxq "source_revision=$ALPINE_GOSU_SOURCE_REVISION" "$gosu_evidence/identity.txt" || fail 'prior Gosu source mismatch'
[[ "$(cat "$gosu_evidence/gosu-sha256.txt")" == "$expected_gosu" ]] || fail 'prior Gosu digest mismatch'
printf '%s  %s\n' "$expected_gosu" "$gosu_scratch/one/gosu" | sha256sum -c - >/dev/null
cmp "$gosu_scratch/one/gosu" "$gosu_scratch/two/gosu" || fail 'prior Gosu builds differ'
cp "$gosu_scratch/one/gosu" "$scratch/context/gosu"
cp ops/db-alpine-gosu-candidate/Dockerfile "$scratch/context/Dockerfile"
cp -R "$gosu_evidence" "$evidence/gosu-build"

stage=identity
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
PYTHONPATH=scripts python3 - "$evidence" <<'PY'
import sys
from pathlib import Path
from capture_db_image_scan import capture_identity, capture_registry_identity
out = Path(sys.argv[1])
capture_identity(out)
capture_registry_identity(out, 'library/postgres',
    'sha256:b6972e1115a4ff8eba5d7f7d19184926d1e16d7d1136ae1e1a7ebf286da0f015',
    'sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366', 'stock-')
PY
[[ "sha256:$(sha256sum "$evidence/stock-config.json" | cut -d ' ' -f 1)" == "$stock_config" ]] || fail 'stock registry config mismatch'
docker pull --platform linux/amd64 "$stock" > "$scratch/stock-pull.log" 2>&1 || fail 'stock image pull failed'
[[ "$(docker image inspect "$stock" --format '{{.Id}}')" == "$stock_config" ]] || fail 'stock image config mismatch'

stage=candidate
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
docker build --network none --platform linux/amd64 --tag "$image" --file "$scratch/context/Dockerfile" "$scratch/context" > "$scratch/image-build.log" 2>&1 || fail 'candidate image build failed'
candidate_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$stock_config" ]] || fail 'candidate image config mismatch'
printf 'stock_config=%s\ncandidate_config=%s\nrebuilt_gosu_sha256=%s\n' "$stock_config" "$candidate_id" "$expected_gosu" >> "$evidence/identity.txt"
installed_sha=$(docker run --rm --network none --entrypoint sha256sum "$image" /usr/local/bin/gosu | awk '{print $1}')
[[ "$installed_sha" == "$expected_gosu" ]] || fail 'candidate executable mismatch'
selected=$(docker run --rm --network none --entrypoint sh "$image" -c 'command -v gosu')
[[ "$selected" == /usr/local/bin/gosu ]] || fail 'unexpected Gosu path'
normalize_groups() {
  local value=$1
  [[ "$value" =~ ^[0-9]+(\ [0-9]+)*$ ]] || return 1
  printf '%s\n' "$value" | tr ' ' '\n' | LC_ALL=C sort -n -u | paste -sd ',' -
}
uid=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -u)
gid=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -g)
groups=$(docker run --rm --network none --entrypoint /usr/local/bin/gosu "$image" postgres id -G)
stock_groups=$(docker run --rm --network none --entrypoint id "$stock" -G postgres)
[[ "$uid" =~ ^[0-9]+$ && "$gid" =~ ^[0-9]+$ ]] || fail 'invalid numeric identity'
group_set=$(normalize_groups "$groups") || fail 'invalid candidate groups'
stock_group_set=$(normalize_groups "$stock_groups") || fail 'invalid stock groups'
printf 'uid=%s\ngid=%s\ncandidate_groups=%s\nstock_groups=%s\n' "$uid" "$gid" "$group_set" "$stock_group_set" > "$evidence/identity-check.txt"
[[ "$uid" == 70 && "$gid" == 70 && "$group_set" == "$stock_group_set" ]] || fail 'postgres identity mismatch'
[[ ",$group_set," != *,0,* && ",$group_set," == *,70,* ]] || fail 'unsafe postgres groups'
if docker run --rm --network none --user 70:70 --entrypoint /usr/local/bin/gosu "$image" root id > "$scratch/denied.log" 2>&1; then
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
[[ "$result" == 5 ]] || fail 'synthetic query mismatch'
printf 'nonroot_to_root=denied\nsynthetic_query=%s\n' "$result" > "$evidence/synthetic-checks.txt"

docker save "$image" -o "$scratch/candidate.tar"
sha256sum "$scratch/candidate.tar" | awk '{print $1}' > "$evidence/candidate-tar-sha256.txt"
python3 scripts/check_alpine_gosu_image.py "$scratch/candidate.tar" "$evidence/stock-config.json" "$candidate_id" "$evidence"

stage=paired_scan
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
printf '{}\n' > "$scratch/empty.yml"
: > "$scratch/empty.ignore"
cp "$scratch/empty.yml" "$evidence/empty.yml"
cp "$scratch/empty.ignore" "$evidence/empty.ignore"
scanner_args=(--config /scratch/empty.yml --cache-dir /scratch/cache image --format json
  --scanners vuln --pkg-types 'os,library' --severity 'UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL'
  --pkg-relationships 'unknown,root,workspace,direct,indirect' --list-all-pkgs
  --show-suppressed --exit-code 0 --ignorefile /scratch/empty.ignore --platform linux/amd64)
scan() {
  local label=$1
  shift
  local -a command=(docker run --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
    --user "$(id -u):$(id -g)" --tmpfs '/tmp:rw,nosuid,nodev' \
    --mount "type=bind,src=$scratch,dst=/scratch" "$scanner" "${scanner_args[@]}" "$@")
  printf '%q ' "${command[@]}" > "$evidence/$label-command.txt"
  printf '\n' >> "$evidence/$label-command.txt"
  "${command[@]}" > "$scratch/$label-scan.log" 2>&1
}
scan stock --image-src remote --output /scratch/stock-report.json "$stock" || fail 'stock scan failed'
cp "$scratch/cache/db/metadata.json" "$evidence/db-metadata.json"
scan candidate --skip-db-update --input /scratch/candidate.tar --output /scratch/candidate-report.json || fail 'candidate scan failed'
cmp "$evidence/db-metadata.json" "$scratch/cache/db/metadata.json"
cp "$scratch/stock-report.json" "$evidence/stock-report.json"
cp "$scratch/candidate-report.json" "$evidence/candidate-report.json"
PYTHONPATH=scripts python3 scripts/check_db_alpine_gosu.py "$evidence" "$candidate_id" > "$evidence/scan-comparison.json"
printf 'stage=complete outcome=success\n' > "$evidence/status.txt"
(cd "$evidence" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
mv "$scratch/SHA256SUMS" "$evidence/SHA256SUMS"
