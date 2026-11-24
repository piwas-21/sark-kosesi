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
mkdir "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stock='mirror.gcr.io/library/postgres@sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366'
stock_config='sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e'
scanner='mirror.gcr.io/aquasec/trivy@sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c'
scanner_config='sha256:c0a2b004a57047aff2bc7a8b87d693d368ba40cd10ef9bb1213345f043f416dd'
expected_gosu=ad2e645f69d82e23e0b54a208828589ff4a021a343ad5090cd36f2a2ae7aa556
db_id=''
stage=gosu_custody
cleanup() {
  local rc=$?
  if [[ "$rc" -ne 0 ]]; then printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$evidence/status.txt"; fi
  if [[ "$stage" == paired_scan && "$rc" -ne 0 ]]; then printf 'outcome=failed\n' > "$evidence/scan-status.txt"; fi
  if [[ "$stage" == application_restore && "$rc" -ne 0 ]]; then printf 'outcome=failed\n' > "$evidence/restore-status.txt"; fi
  if [[ -n "$db_id" ]]; then docker rm -f -v "$db_id" >/dev/null || true; fi
  (cd "$evidence" && find . -type f ! -path './SHA256SUMS' -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$evidence/SHA256SUMS"
}
trap cleanup EXIT
printf 'source_revision=%s\nstock=%s\nscanner=%s\n' "$ALPINE_GOSU_SOURCE_REVISION" "$stock" "$scanner" > "$evidence/identity.txt"
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
printf 'outcome=not-run\n' > "$evidence/scan-status.txt"
printf 'outcome=not-run\n' > "$evidence/restore-status.txt"
gosu_evidence="$RUNNER_TEMP/db-gosu-evidence"
gosu_scratch="$RUNNER_TEMP/db-gosu-scratch"
[[ "$(cat "$gosu_evidence/status.txt")" == 'stage=complete outcome=success' ]] || fail 'prior Gosu build incomplete'
(cd "$gosu_evidence" && sha256sum -c SHA256SUMS >/dev/null) || fail 'prior Gosu evidence checksum mismatch'
grep -Fxq "source_revision=$ALPINE_GOSU_SOURCE_REVISION" "$gosu_evidence/identity.txt" || fail 'prior Gosu source mismatch'
grep -Fxq 'profile=1.20' "$gosu_evidence/identity.txt" || fail 'unexpected Gosu profile'
[[ "$(cat "$gosu_evidence/gosu-sha256.txt")" == "$expected_gosu" ]] || fail 'prior Gosu digest mismatch'
printf '%s  %s\n' "$expected_gosu" "$gosu_scratch/one/gosu" | sha256sum -c - >/dev/null
cmp "$gosu_scratch/one/gosu" "$gosu_scratch/two/gosu" || fail 'prior Gosu builds differ'
[[ "$(grep -c '^candidate_image_id=' "$gosu_evidence/identity.txt")" == 1 ]] || fail 'ambiguous builder image identity'
candidate_id=$(sed -n 's/^candidate_image_id=//p' "$gosu_evidence/identity.txt")
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$stock_config" ]] || fail 'candidate image config mismatch'
[[ "$(docker image inspect "$candidate_id" --format '{{.Id}}')" == "$candidate_id" ]] || fail 'builder image unavailable'
image="$candidate_id"
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
sh ops/db-zlib-candidate/pull_verified_base.sh --image "$stock" "$stock_config" "$scratch/stock-pull.log" \
  > /dev/null 2> "$evidence/stock-pull-status.txt" || fail 'verified stock image pull failed'

stage=candidate
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
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
printf 'outcome=running\n' > "$evidence/scan-status.txt"
sh ops/db-zlib-candidate/pull_verified_base.sh --image "$scanner" "$scanner_config" "$scratch/scanner-pull.log" \
  > /dev/null 2> "$evidence/scanner-pull-status.txt" || fail 'verified scanner pull failed'
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
  local log="$scratch/$label-scan.log" category=success rc=0
  shift
  local -a command=(docker run --pull=never --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
    --user "$(id -u):$(id -g)" --tmpfs '/tmp:rw,nosuid,nodev' \
    --mount "type=bind,src=$scratch,dst=/scratch" "$scanner" "${scanner_args[@]}" "$@")
  printf '%q ' "${command[@]}" > "$evidence/$label-command.txt"
  printf '\n' >> "$evidence/$label-command.txt"
  if (umask 077 && "${command[@]}" > "$log" 2>&1); then
    :
  else
    rc=$?
    category=generic_exit
    if LC_ALL=C awk '
      { line = tolower($0) }
      line ~ /(^|[^[:alnum:]._-])mirror[.]gcr[.]io([:\/[:space:]]|$)/ &&
      line ~ /(^|[^[:alnum:]_])toomanyrequests:|(^|[^[:digit:]])429[[:space:]]+too many requests|status( code)?:?[[:space:]]+429([^[:digit:]]|$)/ { quota = 1 }
      END { exit !quota }
    ' "$log" 2>/dev/null; then
      category=mirror_rate_limit
    fi
  fi
  printf 'category=%s exit=%s\n' "$category" "$rc" > "$evidence/$label-scan-status.txt" || return 1
  return "$rc"
}
scan stock --image-src remote --output /scratch/stock-report.json "$stock" || fail 'stock scan failed'
cp "$scratch/cache/db/metadata.json" "$evidence/db-metadata.json"
cp "$scratch/stock-report.json" "$evidence/stock-report.json"
scan candidate --skip-db-update --input /scratch/candidate.tar --output /scratch/candidate-report.json || fail 'candidate scan failed'
cp "$scratch/candidate-report.json" "$evidence/candidate-report.json"
cmp "$evidence/db-metadata.json" "$scratch/cache/db/metadata.json"
PYTHONPATH=scripts python3 scripts/check_db_alpine_gosu.py "$evidence" "$candidate_id" > "$evidence/scan-comparison.json"
printf 'outcome=success\n' > "$evidence/scan-status.txt"

stage=application_restore
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
printf 'outcome=running\n' > "$evidence/restore-status.txt"
source_id=$(sh ops/db-zlib-candidate/pull_verified_base.sh "$scratch/base-pull.log" \
  2> "$evidence/restore-source-pull-status.txt") || fail 'restore source pull failed'
[[ "$source_id" == sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d ]] || fail 'restore source identity mismatch'
python3 scripts/rehearse_alpine_gosu_restore.py "$candidate_id" "$ALPINE_GOSU_SOURCE_REVISION" "$scratch" "$evidence"
printf 'outcome=success\n' > "$evidence/restore-status.txt"
printf 'stage=complete outcome=success\n' > "$evidence/status.txt"
