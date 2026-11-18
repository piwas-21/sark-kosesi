#!/usr/bin/env bash
# Hosted synthetic TLS lab; keys and credentials remain in disposable scratch.
set -euo pipefail
fail() { printf 'sibling TLS lab: %s\n' "$1" >&2; exit 1; }
: "${RUNNER_TEMP:?}" "${TLS_REVISION:?}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'hosted runner required'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'AMD64 Linux required'
[[ "$TLS_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$TLS_REVISION" ]] || fail 'revision mismatch'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
umask 077
out="$RUNNER_TEMP/db-sibling-tls-evidence"
scratch="$RUNNER_TEMP/db-sibling-tls-scratch"
mkdir "$out" "$scratch" "$scratch/docker-config" "$scratch/private"
export DOCKER_CONFIG="$scratch/docker-config"
private="$scratch/private"
container_id=''
stage=composition
cleanup() {
  rc=$?
  clean=success
  if [[ -n "$container_id" ]] && ! docker rm -f -v "$container_id" >/dev/null 2>&1; then rc=1; clean=failed; fi
  if ! rm -rf "$private"; then rc=1; clean=failed; fi
  if [[ -e "$private" ]]; then rc=1; clean=failed; fi
  if [[ "$rc" == 0 ]]; then printf 'stage=complete outcome=success cleanup=%s\n' "$clean" > "$out/status.txt"; else printf 'stage=%s outcome=failed exit=%s cleanup=%s\n' "$stage" "$rc" "$clean" > "$out/status.txt"; fi
  (cd "$out" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
  mv "$scratch/SHA256SUMS" "$out/SHA256SUMS"
  exit "$rc"
}
trap cleanup EXIT
# The public prerequisite build needs its ordinary readable build-context files.
# Its subprocess cannot change this shell's restrictive TLS-secret umask.
# shellcheck source=ops/db-sibling-postgres-tls/prerequisite.sh
source ops/db-sibling-postgres-tls/prerequisite.sh
run_composition ops/db-sibling-postgres/run.sh "$scratch/composition.log" "$out"
for name in db-sibling-postgres-evidence db-sibling-root-evidence db-source-feature-evidence; do
  grep -Fxq 'stage=complete outcome=success' "$RUNNER_TEMP/$name/status.txt"
  (cd "$RUNNER_TEMP/$name" && sha256sum -c SHA256SUMS >/dev/null)
done
grep -Fxq "revision=$TLS_REVISION" "$RUNNER_TEMP/db-sibling-postgres-evidence/revision.txt"
image="local/db-sibling-postgres:$TLS_REVISION"
image_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]
grep -Fxq "composed=$image_id" "$RUNNER_TEMP/db-sibling-postgres-evidence/image-ids.txt"
printf 'revision=%s\nimage_id=%s\ncomposition_checksums_sha256=%s\nopenssl_sha256=%s\n' "$TLS_REVISION" "$image_id" \
  "$(sha256sum "$RUNNER_TEMP/db-sibling-postgres-evidence/SHA256SUMS" | cut -d ' ' -f 1)" \
  "$(sha256sum "$(command -v openssl)" | cut -d ' ' -f 1)" > "$out/identity.txt"
stage=certificates
# Independent throwaway CAs and a single-DNS-SAN server certificate.
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 -subj '/CN=TLS Lab CA' \
  -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -keyout "$private/ca.key" -out "$private/ca.crt" > "$private/openssl.log" 2>&1
openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 -subj '/CN=TLS Lab Wrong CA' \
  -addext 'basicConstraints=critical,CA:TRUE' -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -keyout "$private/wrong-ca.key" -out "$private/wrong-ca.crt" >> "$private/openssl.log" 2>&1
openssl req -new -newkey rsa:2048 -nodes -sha256 -subj '/CN=sibling-db.test' \
  -keyout "$private/server.key" -out "$private/server.csr" >> "$private/openssl.log" 2>&1
printf '%s\n' 'basicConstraints=critical,CA:FALSE' 'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' 'subjectAltName=DNS:sibling-db.test' > "$private/server.ext"
openssl x509 -req -sha256 -days 1 -set_serial 1 -in "$private/server.csr" -CA "$private/ca.crt" \
  -CAkey "$private/ca.key" -extfile "$private/server.ext" -out "$private/server.crt" >> "$private/openssl.log" 2>&1
openssl verify -purpose sslserver -CAfile "$private/ca.crt" "$private/server.crt" > "$private/verify.log" 2>&1
if openssl verify -purpose sslserver -CAfile "$private/wrong-ca.crt" "$private/server.crt" > "$private/wrong-verify.log" 2>&1; then fail 'wrong CA unexpectedly verifies'; fi
openssl rand -hex 32 > "$private/password"
printf 'ca=%s\nserver=%s\nwrong_ca=%s\n' "$(sha256sum "$private/ca.crt" | cut -d ' ' -f 1)" \
  "$(sha256sum "$private/server.crt" | cut -d ' ' -f 1)" "$(sha256sum "$private/wrong-ca.crt" | cut -d ' ' -f 1)" > "$out/certificate-sha256.txt"
stage=container
container_id=$(docker run -d --network none --user postgres --cap-drop ALL --security-opt=no-new-privileges \
  --tmpfs /tmp:rw,nosuid,nodev,mode=1777,size=256m --memory 768m --cpus 1 --pids-limit 256 \
  --entrypoint sleep "$image_id" 1800)
[[ "$(docker inspect "$container_id" --format '{{.Image}}')" == "$image_id" ]]
[[ "$(docker inspect "$container_id" --format '{{.HostConfig.NetworkMode}}')" == none ]]
bindings=$(docker inspect "$container_id" --format '{{json .HostConfig.PortBindings}}')
[[ "$bindings" == null || "$bindings" == '{}' ]]
docker exec "$container_id" bash -c 'set -eu; test "$(id -u)" = 999; test "$(id -g)" = 999; umask 077; mkdir -m 700 /tmp/sibling-tls'
# shellcheck source=ops/db-source-feature-smoke/transfer.sh
source ops/db-source-feature-smoke/transfer.sh
stage=private_transfer
for name in ca.crt wrong-ca.crt server.crt server.key password; do
  transfer_in "$container_id" "/tmp/sibling-tls/$name" "$private/$name" 16384
done
transfer_in "$container_id" /tmp/sibling-tls/diagnostics.sh ops/db-sibling-postgres-tls/diagnostics.sh 16384
rm -rf "$private"
[[ ! -e "$private" ]]
stage=fixture
docker exec -i "$container_id" bash -s < ops/db-sibling-postgres-tls/fixture.sh > "$out/results.txt" 2> "$scratch/fixture.stderr"
for line in verify_full=pass scram_and_hba=pass wrong_ca=pass wrong_host=pass no_tls=pass wrong_password=pass healthy_after_negatives=pass; do
  grep -Fxq "$line" "$out/results.txt"
done
[[ "$(wc -l < "$out/results.txt")" -eq 7 ]]
