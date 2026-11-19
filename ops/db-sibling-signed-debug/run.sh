#!/usr/bin/env bash
set -euo pipefail
: "${RUNNER_TEMP:?}" "${DEBUG_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]]
[[ "$DEBUG_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$DEBUG_REVISION" ]]
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
scratch="$RUNNER_TEMP/db-sibling-signed-debug-scratch"
published="$RUNNER_TEMP/db-sibling-signed-debug-evidence"
mkdir -m 700 "$scratch" "$scratch/evidence"
out="$scratch/evidence"
stage=lineage
cleanup() {
  local rc=$?
  trap - EXIT
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt" || rc=1; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt" || rc=1; fi
  if (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) > "$scratch/SHA256SUMS" && mv "$scratch/SHA256SUMS" "$out/SHA256SUMS" && [[ ! -e "$published" && ! -L "$published" ]] && mv "$out" "$published"; then :; else rc=1; fi
  exit "$rc"
}
trap cleanup EXIT
LINEAGE_REVISION="$DEBUG_REVISION" bash ops/db-sibling-binary-lineage/run.sh
image=$(docker image inspect "local/db-sibling-postgres:$DEBUG_REVISION" --format '{{.Id}}')
[[ "$image" =~ ^sha256:[0-9a-f]{64}$ ]]
stage=debug
python3 ops/db-sibling-signed-debug/evidence.py "$RUNNER_TEMP/db-sibling-binary-lineage-evidence" "$out" "$scratch" "$DEBUG_REVISION" "$image"
