#!/usr/bin/env bash
# Bounded, disposable hosted image build: no database startup or publication.
set -euo pipefail
fail() { printf 'sibling root: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${SIBLING_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$SIBLING_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$SIBLING_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
out="$RUNNER_TEMP/db-sibling-root-evidence"
scratch="$RUNNER_TEMP/db-sibling-root-scratch"
mkdir "$out" "$scratch" "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
stage=identity
cleanup() {
  rc=$?
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success\n' > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
}
trap cleanup EXIT
printf 'revision=%s\n' "$SIBLING_REVISION" > "$out/revision.txt"
PYTHONPATH=scripts python3 ops/db-sibling-runtime-root/identity.py "$out"
curl -qfsSL --max-time 60 'https://raw.githubusercontent.com/debuerreotype/docker-debian-artifacts/373f61a386722a989c79f0c11f23fcf379b56e94/trixie/slim/rootfs.manifest' > "$out/upstream-packages.txt"
printf '%s  %s\n' f9ef45f4cfc347030cc056cb27360b408cd3753bf1a8faec58946debeb7f2e5d "$out/upstream-packages.txt" | sha256sum -c -
base='mirror.gcr.io/library/debian@sha256:7378260c0a098d27ac206225db5e7d92b8c7c84ea7afccc86eeb513756bbd392'
stock='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
docker pull --platform linux/amd64 "$base" > "$scratch/base-pull.log" 2>&1
docker pull --platform linux/amd64 "$stock" > "$scratch/stock-pull.log" 2>&1
base_id=$(docker image inspect "$base" --format '{{.Id}}')
stock_id=$(docker image inspect "$stock" --format '{{.Id}}')
[[ "$base_id" == sha256:596c6d80c879dc94eae22cb9c59a5e18f66f4f9722e170892324c282b4eb19bc ]] || fail 'root config mismatch'
[[ "$stock_id" == sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d ]] || fail 'stock config mismatch'
stage=build
mkdir "$scratch/build-context"
cp ops/db-sibling-runtime-root/{Dockerfile,packages.lock} "$scratch/build-context/"
cp /etc/ssl/certs/ca-certificates.crt "$scratch/build-context/ca-certificates.crt"
test -s "$scratch/build-context/ca-certificates.crt"
image="local/db-sibling-root:$SIBLING_REVISION"
docker build --platform linux/amd64 -t "$image" -f "$scratch/build-context/Dockerfile" "$scratch/build-context" > "$out/build.log" 2>&1
candidate_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$base_id" ]] || fail 'invalid sibling identity'
docker image inspect "$base_id" > "$out/base-image.json"
docker image inspect "$stock_id" > "$out/stock-image.json"
docker image inspect "$candidate_id" > "$out/candidate-image.json"
printf 'base=%s\nstock=%s\ncandidate=%s\n' "$base_id" "$stock_id" "$candidate_id" > "$out/image-ids.txt"
stage=inventory
for profile in base stock candidate; do
  case "$profile" in base) current=$base_id ;; stock) current=$stock_id ;; candidate) current=$candidate_id ;; esac
  for probe in packages loader; do
    docker run --rm -i --network none --entrypoint sh "$current" -s -- "$probe" < ops/db-sibling-runtime-root/probe.sh > "$out/$profile-$probe.txt"
  done
  if [[ "$profile" != base ]]; then
    for probe in libc locale; do
      docker run --rm -i --network none --entrypoint sh "$current" -s -- "$probe" < ops/db-sibling-runtime-root/probe.sh > "$out/$profile-$probe.txt"
    done
  fi
done
docker run --rm -i --network none --entrypoint sh "$candidate_id" -s -- no-postgres < ops/db-sibling-runtime-root/probe.sh
docker run --rm --network none --entrypoint cat "$candidate_id" /usr/local/share/sibling-root-evidence/trust-inputs.txt > "$out/trust-inputs.txt"
stage=validate
python3 ops/db-sibling-runtime-root/validate.py "$out" > "$out/validation.txt"
