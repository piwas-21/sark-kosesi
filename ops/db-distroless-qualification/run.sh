#!/usr/bin/env bash
set -euo pipefail
set +x
fail() { printf 'Distroless lab: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${QUALIFICATION_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux AMD64 required'
[[ "$QUALIFICATION_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$QUALIFICATION_REVISION" ]] || fail 'exact checkout required'
[[ -z "$(git status --porcelain --untracked-files=all)" ]] || fail 'clean checkout required'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock PYTHONDONTWRITEBYTECODE=1
export PYTHONPATH="$PWD/scripts:$PWD/ops/db-sibling-scan-evidence"
umask 077
lab=ops/db-distroless-qualification
PYTHONPATH="$PWD/$lab" python3 -B - "$RUNNER_TEMP" <<'PY'
import sys
from pathlib import Path
from packages import owned_directory
owned_directory(Path(sys.argv[1]), private=False)
PY
scratch=$(mktemp -d "$RUNNER_TEMP/db-distroless-XXXXXXXX")
published="$RUNNER_TEMP/db-distroless-qualification-evidence"
out="$scratch/evidence"
export DOCKER_CONFIG="$scratch/docker-config"
mkdir -m 700 "$out" "$DOCKER_CONFIG" "$scratch/debian-inputs" "$scratch/private" "$scratch/source-context"
PYTHONPATH="$PWD/$lab" python3 -B - "$scratch" <<'PY'
import sys
from pathlib import Path
from packages import owned_directory
scratch = owned_directory(Path(sys.argv[1]))
for component in ("private", "source-context", "debian-inputs", "docker-config", "evidence"):
    owned_directory(scratch / component)
PY
nonce=$(openssl rand -hex 12)
[[ "$nonce" =~ ^[a-f0-9]{24}$ ]] || fail 'scope creation failed'
source_tag="local/db-distroless-source:$nonce"
server_tag="local/db-distroless-server:$nonce"
tools_tag="local/db-distroless-tools:$nonce"
export_name="sark-distroless-export-$nonce"
source_id='' server_id='' tools_id='' export_id='' daemon_id=''
source_planned=false server_planned=false tools_planned=false export_planned=false runtime_planned=false
stage=preflight
# shellcheck source=ops/db-sibling-postgres-tls/publish.sh
source ops/db-sibling-postgres-tls/publish.sh
custody() {
  local operation=$1 kind=$2 identity=$3
  shift 3
  python3 "$lab/custody.py" "$operation" "$kind" "$identity" "$daemon_id" "$@" 2>> "$scratch/custody.log"
}
export_owned() {
  local found
  found=$(custody owned container "$export_name" --expected "$server_id" --nonce "$nonce") || return 1
  [[ "$found" == "$export_id" ]]
}
finish() {
  local rc=$? clean=success
  trap - EXIT
  if "$export_planned"; then
    custody remove container "$export_name" --expected "$server_id" --nonce "$nonce" \
      --container-id "$export_id" || clean=failed
  fi
  if "$runtime_planned" && [[ ! -f "$scratch/runtime-cleanup.txt" || "$(cat "$scratch/runtime-cleanup.txt")" != success ]]; then clean=failed; fi
  if "$source_planned"; then custody remove image "$source_tag" --expected "$source_id" --nonce "$nonce" || clean=failed; fi
  if "$server_planned"; then custody remove image "$server_tag" --expected "$server_id" || clean=failed; fi
  if "$tools_planned"; then custody remove image "$tools_tag" --expected "$tools_id" || clean=failed; fi
  if [[ "$clean" != success ]]; then rc=1; fi
  if [[ "$rc" == 0 ]]; then
    printf 'stage=complete outcome=success cleanup=%s\n' "$clean" > "$out/status.txt"
  else
    printf 'stage=%s outcome=failed exit=%s cleanup=%s\n' "$stage" "$rc" "$clean" > "$out/status.txt"
  fi
  if ! publish_tls_evidence "$out" "$published"; then rc=1; fi
  if ! rm -rf -- "$scratch"; then rc=1; fi
  exit "$rc"
}
trap finish EXIT
trap 'exit 143' TERM
printf 'revision=%s\n' "$QUALIFICATION_REVISION" > "$out/revision.txt"
printf 'stage=%s\n' "$stage"
df -Pk "$RUNNER_TEMP" | awk 'NR==2 {exit !($4 >= 2097152)}'
for cmd in docker jq gpgv dpkg-deb openssl unshare; do command -v "$cmd" >/dev/null; done
daemon_id=$(docker info --format '{{.ID}}')
[[ -n "$daemon_id" ]] || fail 'Docker daemon identity unavailable'

stage=public_inputs
printf 'stage=%s\n' "$stage"
stock=$(sh ops/db-zlib-candidate/pull_verified_base.sh "$scratch/stock-pull.log")
docker run --rm --pull=never --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
  --user 65534:65534 --entrypoint cat "$stock" /usr/share/keyrings/debian-archive-keyring.gpg \
  > "$scratch/debian-inputs/debian-archive-keyring.gpg"
python3 "$lab/acquire.py" "$scratch" > "$scratch/acquire.log" 2>&1

stage=public_source
printf 'stage=%s\n' "$stage"
build_dir=ops/db-source-feature-smoke
for item in Dockerfile packages.lock; do
  case "$item" in Dockerfile) key=dockerfile_sha256 ;; packages.lock) key=build_packages_sha256 ;; esac
  expected=$(jq -er ".$key" "$lab/source-lock.json")
  printf '%s  %s\n' "$expected" "$build_dir/$item" | sha256sum --check --strict
  install -m 0644 "$build_dir/$item" "$scratch/source-context/$item"
done
install -m 0644 /etc/ssl/certs/ca-certificates.crt "$scratch/source-context/ca-certificates.crt"
custody absent image "$source_tag"
source_planned=true
docker build --platform linux/amd64 --tag "$source_tag" --label "org.piwas.qualification.source=$nonce" \
  "$scratch/source-context" > "$scratch/source-build.log" 2>&1
source_id=$(custody owned image "$source_tag" --nonce "$nonce")
[[ "$source_id" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'invalid source build identity'
for probe in files metadata ownership; do
  docker run --rm --pull=never --network none --read-only --cap-drop ALL --security-opt no-new-privileges \
    --entrypoint /bin/bash -i "$source_id" -s -- "$probe" < ops/db-sibling-postgres/inspect.sh > "$scratch/source-$probe.txt"
done
(ulimit -f 4194304; docker image save --output "$scratch/source-image.tar" "$source_id")
python3 "$lab/source.py" "$scratch" "$source_id"
cp "$scratch/source-receipt.json" "$out/"

stage=assembly
printf 'stage=%s\n' "$stage"
build_lock=$(sha256sum "$lab/assembly-inputs.json" | cut -d ' ' -f 1)
# Network access is confined to input acquisition. The assembler uses this namespace
# to run only GPGV, dpkg-deb, and local OCI layout commands on verified inputs.
(
  sudo --non-interactive unshare --net --setgid "$(id -g)" --setuid "$(id -u)" \
    env RUNNER_TEMP="$RUNNER_TEMP" PYTHONDONTWRITEBYTECODE=1 \
    python3 "$lab/assembly.py" "$build_lock" "$scratch"
) > "$scratch/assembly.log" 2>&1
cp "$scratch/assembly-receipt.json" "$out/"
rm -- "$scratch/source-image.tar"

stage=imports
printf 'stage=%s\n' "$stage"
for role in server tools; do
  manifest=$(jq -er --arg role "$role" '.images[$role].manifest' "$scratch/assembly-receipt.json")
  identity=$(jq -er --arg role "$role" '.images[$role].config' "$scratch/assembly-receipt.json")
  [[ "$manifest" =~ ^sha256:[0-9a-f]{64}$ && "$identity" =~ ^sha256:[0-9a-f]{64}$ ]] || fail 'invalid assembled identity'
  case "$role" in server) tag=$server_tag ;; tools) tag=$tools_tag ;; esac
  (ulimit -f 4194304; "$scratch/regctl-linux-amd64" image export --name "$tag" \
    "ocidir://$scratch/images@$manifest" "$scratch/$role.tar")
  python3 ops/db-sibling-scan-evidence/archive.py "$scratch/$role.tar" "$identity" "$scratch/$role-config.json"
  archive_sha=$(sha256sum "$scratch/$role.tar" | cut -d ' ' -f 1)
  custody absent image "$tag"
  case "$role" in
    server) server_id=$identity; server_planned=true ;;
    tools) tools_id=$identity; tools_planned=true ;;
  esac
  docker image load --input "$scratch/$role.tar" > "$scratch/$role-load.log" 2>&1
  custody owned image "$tag" --expected "$identity" >/dev/null
  [[ "$(sha256sum "$scratch/$role.tar" | cut -d ' ' -f 1)" == "$archive_sha" ]] || fail 'imported archive bytes changed'
  if [[ "$role" == server ]]; then
    jq -cSn --arg image "$identity" --arg sha "$archive_sha" \
      '{server_image:$image,server_image_archive_sha256:$sha}' > "$scratch/server-image-seal.json"
    cp "$scratch/server-image-seal.json" "$out/"
  fi
done
rm -- "$scratch/tools.tar"

stage=isolated_runtime
printf 'stage=%s\n' "$stage"
bash "$lab/certificates.sh" "$scratch/private"
runtime_planned=true
python3 "$lab/run_lab.py" "$server_id" "$tools_id" "$scratch/private" "$out/runtime-results.txt" \
  "$scratch/runtime-cleanup.txt" "$daemon_id" > "$scratch/runtime.log" 2>&1
rm -rf -- "$scratch/private"

stage=paired_scan
printf 'stage=%s\n' "$stage"
custody absent container "$export_name"
export_planned=true
export_id=$(docker create --name "$export_name" --pull=never --network none --read-only \
  --cap-drop ALL --security-opt no-new-privileges --label "org.piwas.qualification.export=$nonce" "$server_id")
if [[ ! "$export_id" =~ ^[a-f0-9]{64}$ ]] || ! export_owned; then
  fail 'root export fixture ownership differs'
fi
(ulimit -f 4194304; docker export --output "$scratch/server-rootfs.tar" "$export_id")
export_owned
jq -cSn --arg image "$server_id" --arg container "$export_id" \
  --arg sha "$(sha256sum "$scratch/server-rootfs.tar" | cut -d ' ' -f 1)" \
  '{server_image:$image,export_container_id:$container,server_rootfs_export_sha256:$sha}' > "$scratch/rootfs-export-seal.json"
cp "$scratch/rootfs-export-seal.json" "$out/"
custody remove container "$export_name" --expected "$server_id" --nonce "$nonce" --container-id "$export_id"
python3 "$lab/scan.py" "$scratch" "$out" "$server_id" > "$scratch/scan.log" 2>&1
