#!/usr/bin/env bash
# Run only on a disposable Linux AMD64 Docker runner. Synthetic data only.
set -euo pipefail
fail() { printf 'zlib candidate: %s\n' "$1" >&2; exit 1; }

: "${ZLIB_EVIDENCE_DIR:?Provide an empty evidence directory}"
: "${ZLIB_SCRATCH_DIR:?Provide an empty scratch directory}"
: "${ZLIB_SOURCE_REVISION:?Provide the checked-out commit ID}"
: "${RUNNER_TEMP:?A GitHub-hosted runner temp directory is required}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'GitHub-hosted runner required'
[[ "$ZLIB_EVIDENCE_DIR" == "$RUNNER_TEMP/db-zlib-evidence" ]] || fail 'unexpected evidence path'
[[ "$ZLIB_SCRATCH_DIR" == "$RUNNER_TEMP/db-zlib-scratch" ]] || fail 'unexpected scratch path'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
[[ "$ZLIB_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ ]] || fail 'invalid source revision'
[[ "$(git rev-parse HEAD)" == "$ZLIB_SOURCE_REVISION" ]] || fail 'source revision mismatch'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux AMD64 required'
mkdir -p "$ZLIB_EVIDENCE_DIR" "$ZLIB_SCRATCH_DIR"
[[ -z "$(ls -A "$ZLIB_EVIDENCE_DIR")" && -z "$(ls -A "$ZLIB_SCRATCH_DIR")" ]] || fail 'output directories must be empty'
evidence=$(cd "$ZLIB_EVIDENCE_DIR" && pwd -P)
scratch=$(cd "$ZLIB_SCRATCH_DIR" && pwd -P)
[[ "$evidence" != "$scratch" && "$scratch" != "$evidence"/* && "$evidence" != "$scratch"/* ]] || fail 'output directories overlap'
image="local/db-zlib-candidate:${ZLIB_SOURCE_REVISION}"
base='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
scanner='mirror.gcr.io/aquasec/trivy@sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c'
source_id=''
target_id=''
copy_id=''
cleanup() {
  for id in "$source_id" "$target_id" "$copy_id"; do
    if [[ -n "$id" ]]; then docker rm -f -v "$id" >/dev/null || true; fi
  done
}
trap cleanup EXIT

printf 'source_revision=%s\nbase=%s\nscanner=%s\n' "$ZLIB_SOURCE_REVISION" "$base" "$scanner" > "$evidence/identity.txt"
mkdir "$scratch/build-context"
cp ops/db-zlib-candidate/Dockerfile "$scratch/build-context/Dockerfile"
cp /etc/ssl/certs/ca-certificates.crt "$scratch/build-context/ca-certificates.crt"
test -s "$scratch/build-context/ca-certificates.crt" || fail 'runner CA bundle is unavailable'
sha256sum "$scratch/build-context/ca-certificates.crt" | awk '{print $1}' > "$evidence/build-ca-sha256.txt"
docker build --pull --platform linux/amd64 --tag "$image" \
  --file "$scratch/build-context/Dockerfile" "$scratch/build-context" > "$evidence/build.log" 2>&1
base_id=$(docker image inspect "$base" --format '{{.Id}}')
candidate_id=$(docker image inspect "$image" --format '{{.Id}}')
[[ "$base_id" == sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d ]] || fail 'unexpected base image identity'
[[ "$candidate_id" =~ ^sha256:[0-9a-f]{64}$ && "$candidate_id" != "$base_id" ]] || fail 'invalid candidate image identity'
printf 'baseline_image_id=%s\ncandidate_image_id=%s\n' "$base_id" "$candidate_id" >> "$evidence/identity.txt"
docker image inspect "$image" > "$evidence/candidate-image-inspect.json"
copy_id=$(docker create --network none "$image")
docker cp "$copy_id:/usr/local/share/db-zlib-candidate-evidence" "$evidence/package-evidence"
docker rm -v "$copy_id" >/dev/null
copy_id=''

start_db() {
  docker run --detach --network none --memory 768m --cpus 1 --pids-limit 256 \
    --tmpfs /var/lib/postgresql:rw,size=256m \
    --env POSTGRES_PASSWORD=isolated-synthetic-password "$image"
}
wait_db() {
  local id=$1
  for ((attempt=0; attempt<60; attempt++)); do
    if docker exec "$id" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  docker logs "$id" >&2
  return 1
}
source_id=$(start_db)
target_id=$(start_db)
wait_db "$source_id"
wait_db "$target_id"
docker exec -i "$source_id" psql -X -v ON_ERROR_STOP=1 -U postgres -d postgres > "$evidence/seed.log" <<'SQL'
CREATE TABLE synthetic_restore (id integer PRIMARY KEY, payload text NOT NULL);
INSERT INTO synthetic_restore VALUES (1, repeat('zlib-pattern-', 2000)), (2, 'Arabic مرحبا and café');
CREATE SEQUENCE synthetic_sequence START WITH 1;
SELECT setval('synthetic_sequence', 170, true);
SQL
docker exec "$source_id" pg_dump -U postgres -d postgres -Fc -Z 6 > "$scratch/synthetic.dump"
test -s "$scratch/synthetic.dump"
shasum -a 256 "$scratch/synthetic.dump" | awk '{print $1}' > "$evidence/synthetic-dump-sha256.txt"
printf 'invalid synthetic archive\n' > "$scratch/invalid.dump"
if docker exec -i "$target_id" pg_restore --list < "$scratch/invalid.dump" > "$evidence/invalid-archive.log" 2>&1; then
  echo 'invalid archive unexpectedly accepted' >&2
  exit 1
fi
docker exec "$target_id" createdb -U postgres restored
docker exec -i "$target_id" pg_restore -U postgres -d restored --exit-on-error < "$scratch/synthetic.dump" > "$evidence/restore.log" 2>&1
actual=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c \
  "SELECT count(*) || '|' || sum(length(payload)) || '|' || (count(*) FILTER (WHERE (id=1 AND payload=repeat('zlib-pattern-', 2000)) OR (id=2 AND payload='Arabic مرحبا and café'))) || '|' || (SELECT last_value || ':' || is_called FROM synthetic_sequence) FROM synthetic_restore")
expected='2|26021|2|170:true'
[[ "$actual" == "$expected" ]] || { printf 'restore mismatch: %s\n' "$actual" >&2; exit 1; }
next=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c "SELECT nextval('synthetic_sequence')")
[[ "$next" == '171' ]] || fail 'restored sequence mismatch'
printf 'rows_and_sequence=%s\nnext_sequence_value=%s\ninvalid_archive=rejected\n' "$actual" "$next" > "$evidence/restore-result.txt"

docker save "$image" -o "$scratch/candidate.tar"
shasum -a 256 "$scratch/candidate.tar" | awk '{print $1}' > "$evidence/candidate-tar-sha256.txt"
printf '{}\n' > "$scratch/empty.yml"
: > "$scratch/empty.ignore"
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
  "${command[@]}"
}
scan base --image-src remote --output /scratch/base.json "$base" > "$evidence/base-scan.log" 2>&1
cp "$scratch/cache/db/metadata.json" "$evidence/db-metadata.json"
scan candidate --skip-db-update --input /scratch/candidate.tar --output /scratch/candidate.json > "$evidence/candidate-scan.log" 2>&1
cmp "$evidence/db-metadata.json" "$scratch/cache/db/metadata.json"
cp "$scratch/base.json" "$evidence/base-report.json"
cp "$scratch/candidate.json" "$evidence/candidate-report.json"
python3 scripts/compare_db_zlib_scan.py "$evidence/base-report.json" "$evidence/candidate-report.json" \
  "$candidate_id" "$evidence/db-metadata.json" > "$evidence/scan-comparison.json"
(cd "$evidence" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
mv "$scratch/SHA256SUMS" "$evidence/SHA256SUMS"
