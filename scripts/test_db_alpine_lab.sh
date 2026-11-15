#!/usr/bin/env bash
# Hosted disposable lab: no original volume, real data or published database port.
set -Eeuo pipefail
fail() { printf 'alpine lab: %s\n' "$1" >&2; exit 1; }
: "${ALPINE_EVIDENCE_DIR:?hosted evidence directory required}"
: "${ALPINE_SCRATCH_DIR:?hosted scratch directory required}"
: "${ALPINE_SOURCE_REVISION:?source revision required}"
: "${RUNNER_TEMP:?hosted runner temp required}"
[[ "${GITHUB_ACTIONS:-}" == true && "${RUNNER_ENVIRONMENT:-}" == github-hosted ]] || fail 'GitHub-hosted runner required'
[[ "$ALPINE_EVIDENCE_DIR" == "$RUNNER_TEMP/db-alpine-evidence" && "$ALPINE_SCRATCH_DIR" == "$RUNNER_TEMP/db-alpine-scratch" ]] || fail 'unexpected output path'
[[ "$ALPINE_SOURCE_REVISION" =~ ^[0-9a-f]{40}$ && "$(git rev-parse HEAD)" == "$ALPINE_SOURCE_REVISION" ]] || fail 'source revision mismatch'
[[ "$(uname -s):$(uname -m)" == Linux:x86_64 ]] || fail 'Linux AMD64 required'
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
export DOCKER_HOST=unix:///var/run/docker.sock
mkdir -p "$ALPINE_EVIDENCE_DIR" "$ALPINE_SCRATCH_DIR"
[[ -z "$(ls -A "$ALPINE_EVIDENCE_DIR")" && -z "$(ls -A "$ALPINE_SCRATCH_DIR")" ]] || fail 'output directories must be empty'
evidence=$(cd "$ALPINE_EVIDENCE_DIR" && pwd -P)
scratch=$(cd "$ALPINE_SCRATCH_DIR" && pwd -P)
[[ "$evidence" != "$scratch" && "$scratch" != "$evidence"/* && "$evidence" != "$scratch"/* ]] || fail 'overlapping output directories'
mkdir "$scratch/docker-config"
export DOCKER_CONFIG="$scratch/docker-config"
base='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
alpine='mirror.gcr.io/library/postgres@sha256:fa1b61f5a4844f4d4b73c9b15716cfc75fe70c931f2d32723630f3b900056366'
scanner='mirror.gcr.io/aquasec/trivy@sha256:85e87be1a96459c38a4eea47dc64eb2d342bb14cd4b4cef96adcf6ff03378b7c'
source_id=''
target_id=''
stage=identity
cleanup() {
  local rc=$?
  if [[ "$rc" -ne 0 ]]; then
    printf 'stage=%s outcome=failed exit=%s\n' "$stage" "$rc" > "$evidence/status.txt"
  fi
  for id in "$source_id" "$target_id"; do
    if [[ -n "$id" ]]; then docker rm -f -v "$id" >/dev/null || true; fi
  done
}
trap cleanup EXIT
printf 'source_revision=%s\nbase=%s\nalpine=%s\nscanner=%s\n' "$ALPINE_SOURCE_REVISION" "$base" "$alpine" "$scanner" > "$evidence/identity.txt"
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
python3 scripts/check_db_alpine_lab.py identity "$evidence"
base_id=$(sh ops/db-zlib-candidate/pull_verified_base.sh "$scratch/base-pull.log") || fail 'base pull or identity check failed'
[[ "$base_id" == sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d ]] || fail 'base identity mismatch'
docker pull --platform linux/amd64 "$alpine" > "$scratch/alpine-pull.log" 2>&1 || fail 'Alpine image pull failed'
alpine_id=$(docker image inspect "$alpine" --format '{{.Id}}')
[[ "$alpine_id" == sha256:0f6c51e48b3458e87ab43692e49c9dafc72ffe0a0dbde3fd8cdb6ab92d5c025e ]] || fail 'Alpine identity mismatch'
printf 'debian_config=%s\nalpine_config=%s\n' "$base_id" "$alpine_id" >> "$evidence/identity.txt"
base_uid=$(docker run --rm --network none --entrypoint id "$base" -u postgres)
base_gid=$(docker run --rm --network none --entrypoint id "$base" -g postgres)
alpine_uid=$(docker run --rm --network none --entrypoint id "$alpine" -u postgres)
alpine_gid=$(docker run --rm --network none --entrypoint id "$alpine" -g postgres)
[[ "$base_uid:$base_gid" == 999:999 && "$alpine_uid:$alpine_gid" == 70:70 ]] || fail 'unexpected image user identity'
printf 'debian_postgres_uid_gid=%s:%s\nalpine_postgres_uid_gid=%s:%s\n' "$base_uid" "$base_gid" "$alpine_uid" "$alpine_gid" > "$evidence/os-identity.txt"

stage=synthetic_restore
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
start_db() {
  docker run --detach --network none --memory 768m --cpus 1 --pids-limit 256 \
    --tmpfs /var/lib/postgresql:rw,size=256m --env POSTGRES_PASSWORD=isolated-synthetic-password "$1"
}
wait_db() {
  local id=$1
  for ((attempt=0; attempt<60; attempt++)); do
    if docker exec "$id" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  fail 'synthetic database startup timed out'
}
source_id=$(start_db "$base")
target_id=$(start_db "$alpine")
wait_db "$source_id"
wait_db "$target_id"
docker exec -i "$source_id" psql -X -v ON_ERROR_STOP=1 -U postgres -d postgres > "$evidence/seed.log" <<'SQL'
CREATE TABLE synthetic_items (id integer PRIMARY KEY, identifier text NOT NULL);
CREATE UNIQUE INDEX synthetic_identifier_c ON synthetic_items (identifier COLLATE "C");
INSERT INTO synthetic_items VALUES (1,'zeta'), (2,'çilek'), (3,'alpha'), (4,'عربي');
CREATE SEQUENCE synthetic_sequence START WITH 1;
SELECT setval('synthetic_sequence',170,true);
SQL
locale_sql="SELECT pg_encoding_to_char(encoding), datlocprovider, COALESCE(datlocale,''), datcollate, datctype, COALESCE(datcollversion,'') FROM pg_database WHERE datname=current_database()"
docker exec "$source_id" psql -X -A -t -F '|' -v ON_ERROR_STOP=1 -U postgres -d postgres -c "$locale_sql" > "$evidence/debian-locale.txt"
docker exec "$source_id" pg_dump -U postgres -d postgres -Fc -Z 6 > "$scratch/synthetic.dump"
test -s "$scratch/synthetic.dump" || fail 'empty synthetic archive'
sha256sum "$scratch/synthetic.dump" | awk '{print $1}' > "$evidence/synthetic-dump-sha256.txt"
printf 'invalid synthetic archive\n' > "$scratch/invalid.dump"
if docker exec -i "$target_id" pg_restore --list < "$scratch/invalid.dump" > "$scratch/invalid-archive.log" 2>&1; then
  fail 'invalid archive unexpectedly accepted'
fi
docker exec "$target_id" createdb -U postgres -T template0 restored
docker exec -i "$target_id" pg_restore -U postgres -d restored --exit-on-error < "$scratch/synthetic.dump" > "$scratch/restore.log" 2>&1
docker exec "$target_id" psql -X -A -t -F '|' -v ON_ERROR_STOP=1 -U postgres -d restored -c "$locale_sql" > "$evidence/alpine-locale.txt"
for file in "$evidence/debian-locale.txt" "$evidence/alpine-locale.txt"; do
  [[ "$(cut -d '|' -f 1 "$file")" == UTF8 ]] || fail 'unexpected database encoding'
done
actual=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c \
  "SELECT count(*) || '|' || (count(*) FILTER (WHERE (id=1 AND identifier='zeta') OR (id=2 AND identifier='çilek') OR (id=3 AND identifier='alpha') OR (id=4 AND identifier='عربي'))) || '|' || (SELECT last_value || ':' || is_called FROM synthetic_sequence) FROM synthetic_items")
[[ "$actual" == '4|4|170:true' ]] || fail 'restored synthetic rows or sequence mismatch'
order=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c "SELECT string_agg(identifier, '|' ORDER BY identifier COLLATE \"C\") FROM synthetic_items")
[[ "$order" == 'alpha|zeta|çilek|عربي' ]] || fail 'C-collated ordering mismatch'
index=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c \
  "SELECT i.indisunique AND i.indisvalid AND a.attname='identifier' AND c.collname='C' AND c.collnamespace='pg_catalog'::regnamespace FROM pg_index i JOIN pg_class x ON x.oid=i.indexrelid JOIN pg_namespace n ON n.oid=x.relnamespace JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=i.indkey[0] JOIN pg_collation c ON c.oid=i.indcollation[0] WHERE n.nspname='public' AND x.relname='synthetic_identifier_c'")
[[ "$index" == t ]] || fail 'synthetic C unique index mismatch'
docker exec -i "$target_id" psql -X -v ON_ERROR_STOP=1 -U postgres -d restored > "$scratch/duplicate.log" 2>&1 <<'SQL'
DO $$ BEGIN
  BEGIN
    INSERT INTO synthetic_items VALUES (5,'alpha');
    RAISE EXCEPTION 'duplicate unexpectedly accepted';
  EXCEPTION WHEN unique_violation THEN NULL;
  END;
END $$;
SQL
next=$(docker exec "$target_id" psql -X -A -t -v ON_ERROR_STOP=1 -U postgres -d restored -c "SELECT nextval('synthetic_sequence')")
[[ "$next" == 171 ]] || fail 'restored sequence next value mismatch'
printf 'exact_rows_and_sequence=%s\nC_order=%s\nC_unique_index=%s\nduplicate=rejected\ninvalid_archive=rejected\nnext_sequence=%s\n' "$actual" "$order" "$index" "$next" > "$evidence/restore-result.txt"

stage=paired_scan
printf 'stage=%s outcome=running\n' "$stage" > "$evidence/status.txt"
printf '{}\n' > "$scratch/empty.yml"
: > "$scratch/empty.ignore"
cp "$scratch/empty.yml" "$evidence/empty.yml"
cp "$scratch/empty.ignore" "$evidence/empty.ignore"
scanner_args=(--config /scratch/empty.yml --cache-dir /scratch/cache image --format json
  --scanners vuln --pkg-types 'os,library' --severity 'UNKNOWN,LOW,MEDIUM,HIGH,CRITICAL'
  --pkg-relationships 'unknown,root,workspace,direct,indirect' --list-all-pkgs
  --show-suppressed --exit-code 0 --ignorefile /scratch/empty.ignore --platform linux/amd64 --image-src remote)
scan() {
  local label=$1
  shift
  local -a command=(docker run --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
    --user "$(id -u):$(id -g)" --tmpfs '/tmp:rw,nosuid,nodev' \
    --mount "type=bind,src=$scratch,dst=/scratch" "$scanner" "${scanner_args[@]}" "$@")
  printf '%q ' "${command[@]}" > "$evidence/$label-command.txt"
  printf '\n' >> "$evidence/$label-command.txt"
  "${command[@]}" > "$scratch/$label-scan.log" 2>&1
}
scan base --output /scratch/base-report.json "$base" || fail 'base scan failed'
cp "$scratch/cache/db/metadata.json" "$evidence/db-metadata.json"
scan alpine --skip-db-update --output /scratch/alpine-report.json "$alpine" || fail 'Alpine scan failed'
cmp "$evidence/db-metadata.json" "$scratch/cache/db/metadata.json"
cp "$scratch/base-report.json" "$evidence/base-report.json"
cp "$scratch/alpine-report.json" "$evidence/alpine-report.json"
python3 scripts/check_db_alpine_lab.py compare "$evidence" > "$evidence/scan-comparison.json"
printf 'stage=complete outcome=success\n' > "$evidence/status.txt"
(cd "$evidence" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum > "$scratch/SHA256SUMS")
mv "$scratch/SHA256SUMS" "$evidence/SHA256SUMS"
