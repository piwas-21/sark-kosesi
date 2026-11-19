#!/usr/bin/env bash
# Same-run Debian signed package lineage; never execute the image for this check.
set -euo pipefail
: "${RUNNER_TEMP:?}" "${LINEAGE_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]]
[[ "$LINEAGE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$LINEAGE_REVISION" ]]
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
published="$RUNNER_TEMP/db-sibling-binary-lineage-evidence"
scratch="$RUNNER_TEMP/db-sibling-binary-lineage-scratch"
out="$scratch/evidence"
mkdir -m 700 "$scratch" "$out" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stage=presence
cleanup() {
  local rc=$? written=1
  trap - EXIT
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt" || { rc=1; written=0; }; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt" || { rc=1; written=0; }; fi
  if [[ "$written" == 1 ]]; then
    if (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) > "$scratch/SHA256SUMS" &&
       mv "$scratch/SHA256SUMS" "$out/SHA256SUMS" && [[ ! -e "$published" && ! -L "$published" ]] && mv "$out" "$published"; then :; else rc=1; fi
  fi
  exit "$rc"
}
trap cleanup EXIT
PRESENCE_REVISION="$LINEAGE_REVISION" bash ops/db-sibling-component-presence/run.sh
presence="$RUNNER_TEMP/db-sibling-component-presence-evidence"
python3 - "$presence" <<'PY'
import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location('custody', 'ops/db-sibling-component-presence/custody.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.verify(Path(sys.argv[1]), {'identity.txt', 'report.json', 'status.txt'})
PY
image=$(docker image inspect "local/db-sibling-postgres:$LINEAGE_REVISION" --format '{{.Id}}')
[[ "$image" =~ ^sha256:[0-9a-f]{64}$ ]]
grep -Fxq "revision=$LINEAGE_REVISION" "$presence/identity.txt"
grep -Fxq "image_id=$image" "$presence/identity.txt"
python3 - "$presence/report.json" "$LINEAGE_REVISION" "$image" <<'PY'
import json, sys
report = json.load(open(sys.argv[1], encoding='utf-8'))
if report['revision'] != sys.argv[2] or report['image_id'] != sys.argv[3]:
    raise ValueError('presence identity mismatch')
PY
printf 'revision=%s\nimage_id=%s\npresence_manifest_sha256=%s\n' "$LINEAGE_REVISION" "$image" "$(sha256sum "$presence/SHA256SUMS" | cut -d ' ' -f 1)" > "$out/identity.txt"
stage=lineage
python3 ops/db-sibling-binary-lineage/lineage.py \
  "$RUNNER_TEMP/db-sibling-component-presence-scratch/rootfs.tar" "$presence/report.json" \
  "$RUNNER_TEMP/db-sibling-root-evidence" "$scratch" "$out"
