#!/usr/bin/env bash
# Static inspection of an unstarted, unmounted same-run composed image.
set -euo pipefail
: "${RUNNER_TEMP:?}" "${PRESENCE_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]]
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]]
[[ "$PRESENCE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$PRESENCE_REVISION" ]]
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
published="$RUNNER_TEMP/db-sibling-component-presence-evidence"
scratch="$RUNNER_TEMP/db-sibling-component-presence-scratch"
out="$scratch/evidence"
mkdir -m 700 "$scratch" "$out" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stage=scan
container=''
cleanup() {
  local rc=$? written=1
  trap - EXIT
  if [[ -n "$container" ]]; then docker rm -v "$container" >/dev/null || rc=1; fi
  if [[ "$rc" == 0 ]]; then
    printf 'stage=complete outcome=success\n' > "$out/status.txt" || { rc=1; written=0; }
  else
    printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt" || { rc=1; written=0; }
  fi
  if [[ "$written" == 1 ]]; then
    if (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) > "$scratch/SHA256SUMS" &&
       mv "$scratch/SHA256SUMS" "$out/SHA256SUMS" && [[ ! -e "$published" && ! -L "$published" ]] && mv "$out" "$published"; then :; else rc=1; fi
  fi
  exit "$rc"
}
trap cleanup EXIT
SCAN_REVISION="$PRESENCE_REVISION" bash ops/db-sibling-scan-evidence/run.sh
python3 ops/db-sibling-component-presence/custody.py "$RUNNER_TEMP"
scan="$RUNNER_TEMP/db-sibling-scan-evidence"
composition="$RUNNER_TEMP/db-sibling-postgres-evidence"
grep -Fxq "revision=$PRESENCE_REVISION" "$scan/revision.txt"
grep -Fxq "revision=$PRESENCE_REVISION" "$composition/revision.txt"
image=$(docker image inspect "local/db-sibling-postgres:$PRESENCE_REVISION" --format '{{.Id}}')
[[ "$image" =~ ^sha256:[0-9a-f]{64}$ ]]
grep -Fxq "composed=$image" "$scan/identity.txt"
grep -Fxq "composed=$image" "$composition/image-ids.txt"
python3 - "$scan" "$image" <<'PY'
import hashlib, json, sys
from pathlib import Path
path, image = Path(sys.argv[1]), sys.argv[2]
if (json.loads((path / 'comparison.json').read_text())['composed_image_id'] != image
        or json.loads((path / 'source-component.json').read_text())['composed_image_id'] != image
        or 'sha256:' + hashlib.sha256((path / 'composed-config.json').read_bytes()).hexdigest() != image):
    raise ValueError('scan identity mismatch')
PY
printf 'revision=%s\nimage_id=%s\nscan_manifest_sha256=%s\ncomposition_manifest_sha256=%s\n' "$PRESENCE_REVISION" "$image" \
  "$(sha256sum "$scan/SHA256SUMS" | cut -d ' ' -f 1)" \
  "$(sha256sum "$composition/SHA256SUMS" | cut -d ' ' -f 1)" > "$out/identity.txt"
stage=create
container=$(docker create --network none --entrypoint /not-executed "$image")
[[ "$container" =~ ^[0-9a-f]{64}$ ]]
docker inspect "$container" > "$scratch/before.json"
python3 - "$scratch/before.json" "$image" "$container" <<'PY'
import importlib.util, json, sys
spec = importlib.util.spec_from_file_location('rootfs_inspection', 'ops/db-sibling-component-presence/inspect_rootfs.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
container = json.load(open(sys.argv[1], encoding='utf-8'))
module.validate_container(container, sys.argv[2])
if container[0].get('Id') != sys.argv[3]:
    raise ValueError('created container identity mismatch')
PY
stage='export'
# The export stays in runner scratch; the daemon never starts the container.
(ulimit -f 2097152; timeout 300 docker export --output "$scratch/rootfs.tar" "$container")
docker inspect "$container" > "$scratch/after.json"
stage=inspect
python3 ops/db-sibling-component-presence/inspect_rootfs.py "$scratch/rootfs.tar" "$image" "$PRESENCE_REVISION" "$scratch/before.json" "$scratch/after.json" "$out/report.json"
