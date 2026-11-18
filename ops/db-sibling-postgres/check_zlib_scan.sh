#!/usr/bin/env bash
# Validate only evidence created by the same successful hosted paired scan.
set -euo pipefail
: "${RUNNER_TEMP:?}" "${ZLIB_CHECK_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]
[[ "$ZLIB_CHECK_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$ZLIB_CHECK_REVISION" ]]
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-sibling-zlib-check-evidence"
scratch="$RUNNER_TEMP/db-sibling-zlib-check-scratch"
scan="$RUNNER_TEMP/db-sibling-scan-evidence"
composition="$RUNNER_TEMP/db-sibling-postgres-evidence"
mkdir "$out" "$scratch" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stage=inputs
cleanup() {
  rc=$?
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
}
trap cleanup EXIT
grep -Fxq 'stage=complete outcome=success' "$scan/status.txt"
(cd "$scan" && sha256sum -c SHA256SUMS >/dev/null)
grep -Fxq "revision=$ZLIB_CHECK_REVISION" "$scan/revision.txt"
grep -Fxq "revision=$ZLIB_CHECK_REVISION" "$composition/revision.txt"
candidate=$(docker image inspect "local/db-sibling-postgres:$ZLIB_CHECK_REVISION" --format '{{.Id}}')
[[ "$candidate" =~ ^sha256:[0-9a-f]{64}$ ]]
grep -Fxq "composed=$candidate" "$scan/identity.txt"
grep -Fxq "composed=$candidate" "$composition/image-ids.txt"
stage=target_check
PYTHONPATH=scripts python3 ops/db-sibling-postgres/check_zlib_scan.py "$scan" "$candidate" > "$out/result.json"
