#!/usr/bin/env bash
# Paired, unfiltered evidence only; no publication or vulnerability acceptance.
set -euo pipefail
fail() { printf 'sibling scan: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${SCAN_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$SCAN_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$SCAN_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-sibling-scan-evidence"
scratch="$RUNNER_TEMP/db-sibling-scan-scratch"
mkdir "$out" "$scratch" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stage=composition
cleanup() {
  rc=$?
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
}
trap cleanup EXIT
printf 'revision=%s\n' "$SCAN_REVISION" > "$out/revision.txt"
COMPOSE_REVISION="$SCAN_REVISION" bash ops/db-sibling-postgres/run.sh
for item in db-sibling-postgres-evidence db-sibling-root-evidence db-source-feature-evidence; do
  grep -Fxq 'stage=complete outcome=success' "$RUNNER_TEMP/$item/status.txt"
  (cd "$RUNNER_TEMP/$item" && sha256sum -c SHA256SUMS >/dev/null)
done
grep -Fxq "revision=$SCAN_REVISION" "$RUNNER_TEMP/db-sibling-postgres-evidence/revision.txt"
candidate="local/db-sibling-postgres:$SCAN_REVISION"
candidate_id=$(docker image inspect "$candidate" --format '{{.Id}}')
grep -Fxq "composed=$candidate_id" "$RUNNER_TEMP/db-sibling-postgres-evidence/image-ids.txt"
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'invalid composition identity'
stock='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
scanner='mirror.gcr.io/aquasec/trivy@sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c'
printf 'revision=%s\nstock=%s\ncomposed=%s\nscanner=%s\n' "$SCAN_REVISION" "$stock" "$candidate_id" "$scanner" > "$out/identity.txt"
stage=identities
PYTHONPATH=scripts python3 - "$out" <<'PY'
import sys
from pathlib import Path
from capture_db_image_scan import capture_identity
capture_identity(Path(sys.argv[1]))
PY
mv "$out/config.json" "$out/stock-config.json"
docker pull --platform linux/amd64 "$scanner" > "$scratch/scanner-pull.log" 2>&1
scanner_id=$(docker image inspect "$scanner" --format '{{.Id}}')
expected_scanner=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["config"]["digest"])' "$out/scanner-manifest.json")
[[ "$scanner_id" == "$expected_scanner" ]] || fail 'scanner image identity mismatch'
docker run --rm --network none --entrypoint /opt/pg-smoke/bin/postgres "$candidate_id" --version > "$out/postgres-version.txt"
docker run --rm --network none --entrypoint /opt/pg-smoke/bin/pg_config "$candidate_id" --version > "$out/pg-config-version.txt"
python3 ops/db-sibling-scan-evidence/source_manifest.py "$RUNNER_TEMP/db-source-feature-evidence" "$RUNNER_TEMP/db-sibling-postgres-evidence" "$out" "$SCAN_REVISION" "$candidate_id"
stage=archive
docker image save -o "$scratch/composed.tar" "$candidate_id"
test "$(stat -c %s "$scratch/composed.tar")" -le 2147483648
sha256sum "$scratch/composed.tar" | cut -d ' ' -f 1 > "$out/composed-archive-sha256.txt"
PYTHONPATH=scripts python3 ops/db-sibling-scan-evidence/archive.py "$scratch/composed.tar" "$candidate_id" "$out/composed-config.json"
stage=scans
printf '{}\n' > "$scratch/empty.yml"
: > "$scratch/empty.ignore"
cp "$scratch/empty.yml" "$out/empty.yml"
cp "$scratch/empty.ignore" "$out/empty.ignore"
scanner_args=(--config /scratch/empty.yml --cache-dir /scratch/cache image --format json
  --scanners vuln --pkg-types 'os,library' --severity 'UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL'
  --pkg-relationships 'unknown,root,workspace,direct,indirect' --list-all-pkgs
  --show-suppressed --exit-code 0 --ignorefile /scratch/empty.ignore --platform linux/amd64)
scan() {
  local label=$1 network=$2
  shift 2
  local -a command=(docker run --rm --network "$network" --read-only --cap-drop=ALL --security-opt=no-new-privileges
    --user "$(id -u):$(id -g)" --tmpfs '/tmp:rw,nosuid,nodev' --mount "type=bind,src=$scratch,dst=/scratch"
    "$scanner" "${scanner_args[@]}" "$@")
  printf '%q ' "${command[@]}" > "$out/$label-command.txt"
  printf '\n' >> "$out/$label-command.txt"
  "${command[@]}"
}
scan stock bridge --image-src remote --output /scratch/stock-report.json "$stock" > "$out/stock-scan.log" 2>&1
cp "$scratch/cache/db/metadata.json" "$out/db-metadata.json"
sha256sum "$scratch/cache/db/trivy.db" | cut -d ' ' -f 1 > "$out/db-sha256.txt"
scan composed none --skip-db-update --input /scratch/composed.tar --output /scratch/composed-report.json > "$out/composed-scan.log" 2>&1
cmp "$out/db-metadata.json" "$scratch/cache/db/metadata.json"
[[ "$(sha256sum "$scratch/cache/db/trivy.db" | cut -d ' ' -f 1)" == "$(cat "$out/db-sha256.txt")" ]] || fail 'database bytes changed'
cp "$scratch/stock-report.json" "$out/stock-report.json"
cp "$scratch/composed-report.json" "$out/composed-report.json"
stage=sbom
for label in stock composed; do
  command=(docker run --rm --network none --read-only --cap-drop=ALL --security-opt=no-new-privileges
    --user "$(id -u):$(id -g)" --tmpfs '/tmp:rw,nosuid,nodev' --mount "type=bind,src=$scratch,dst=/scratch"
    "$scanner" --config /scratch/empty.yml --cache-dir /scratch/cache convert --format cyclonedx
    --ignorefile /scratch/empty.ignore --show-suppressed --list-all-pkgs --severity 'UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL'
    --output "/scratch/$label-sbom.json" "/scratch/$label-report.json")
  printf '%q ' "${command[@]}" > "$out/$label-convert-command.txt"
  printf '\n' >> "$out/$label-convert-command.txt"
  "${command[@]}" > "$out/$label-convert.log" 2>&1
  cp "$scratch/$label-sbom.json" "$out/$label-sbom.json"
done
cmp "$out/db-metadata.json" "$scratch/cache/db/metadata.json"
[[ "$(sha256sum "$scratch/cache/db/trivy.db" | cut -d ' ' -f 1)" == "$(cat "$out/db-sha256.txt")" ]] || fail 'database bytes changed during conversion'
stage=compare
cp "$RUNNER_TEMP/db-sibling-root-evidence/stock-packages.txt" "$out/stock-dpkg.txt"
cp "$RUNNER_TEMP/db-sibling-postgres-evidence/composed-packages.txt" "$out/composed-dpkg.txt"
PYTHONPATH=scripts python3 ops/db-sibling-scan-evidence/compare.py "$out" "$candidate_id"
