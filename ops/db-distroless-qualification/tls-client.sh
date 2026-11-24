#!/usr/bin/env bash
# Run only in the verified client container sharing the owned server namespace.
set +x
set -euo pipefail
export LC_ALL=C PATH=/opt/pg-smoke/bin:/usr/bin:/bin
umask 077
work=/tmp/sibling-tls
[[ "$(id -u):$(id -g)" == 65532:65532 ]] || exit 1
[[ -d "$work" && ! -L "$work" ]] || exit 1
[[ "$(stat -c '%u:%g:%a' "$work")" == '65532:65532:700' ]] || exit 1
for input in password ca.crt wrong-ca.crt diagnostics.sh; do
  file="$work/$input"
  [[ -f "$file" && ! -L "$file" && "$(stat -c '%u:%g:%a' "$file")" == '65532:65532:600' ]]
  [[ "$(wc -c < "$file")" -le 16384 ]]
done
printf '2756980e89637c397051a129eb94a036f9e021803fb8069528972432d638827c  %s/diagnostics.sh\n' "$work" \
  | sha256sum -c - >/dev/null
# shellcheck source=/dev/null
source "$work/diagnostics.sh"
[[ "$(wc -c < "$work/password")" -eq 65 ]]
IFS= read -r password < "$work/password"
[[ "$password" =~ ^[a-f0-9]{64}$ ]]
printf '*:*:*:postgres:%s\n' "$password" > "$work/pgpass"
printf '*:*:*:postgres:wrong-%s\n' "$password" > "$work/badpass"
unset password PGPASSWORD PGSERVICE PGSERVICEFILE PGOPTIONS
export PGHOST=sibling-db.test PGHOSTADDR=127.0.0.1 PGPORT=5432 PGUSER=postgres PGDATABASE=postgres
export PGSSLMODE=verify-full PGSSLROOTCERT="$work/ca.crt" PGPASSFILE="$work/pgpass"
export PGCONNECT_TIMEOUT=3 PGGSSENCMODE=disable
healthy() {
  local reply
  reply=$(psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT ssl AND version IS NOT NULL FROM pg_stat_ssl WHERE pid = pg_backend_pid()' 2> "$work/healthy.err") || return 1
  [[ "$reply" == t ]]
}
for _ in {1..30}; do
  if healthy; then break; fi
  sleep 1
done
healthy
printf 'verify_full=pass\n'
[[ "$(psql -XAtw -v ON_ERROR_STOP=1 -c "SELECT rolpassword LIKE 'SCRAM-SHA-256' || chr(36) || '%' FROM pg_authid WHERE rolname='postgres'" 2> "$work/scram.err")" == t ]]
[[ "$(psql -XAtw -v ON_ERROR_STOP=1 -c "SELECT count(*) = 3 AND count(*) FILTER (WHERE type='hostssl' AND auth_method='scram-sha-256' AND error IS NULL) = 1 AND count(*) FILTER (WHERE type='hostnossl' AND auth_method='reject' AND error IS NULL) = 1 FROM pg_hba_file_rules" 2> "$work/hba.err")" == t ]]
printf 'scram_and_hba=pass\n'
negative() {
  local kind=$1 rc=0
  shift
  "$@" > "$work/$kind.out" 2> "$work/$kind.err" || rc=$?
  check_failure "$kind" "$rc" "$work/$kind.err"
  healthy
  printf '%s=pass\n' "$kind"
}
negative wrong_ca env PGSSLROOTCERT="$work/wrong-ca.crt" psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT 1'
negative wrong_host env PGHOST=other-db.test psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT 1'
negative no_tls env PGSSLMODE=disable psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT 1'
negative wrong_password env PGPASSFILE="$work/badpass" psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT 1'
case "${1:?}" in
  first)
    psql -Xqw -v ON_ERROR_STOP=1 > "$work/transaction.log" 2>&1 <<'SQL'
BEGIN;
CREATE TABLE lifecycle_fixture (id integer PRIMARY KEY, label text NOT NULL);
INSERT INTO lifecycle_fixture VALUES (1, 'committed');
COMMIT;
BEGIN;
INSERT INTO lifecycle_fixture VALUES (2, 'rolled-back');
ROLLBACK;
SQL
    ;;
  restart) ;;
  *) exit 1 ;;
esac
[[ "$(psql -XAtw -v ON_ERROR_STOP=1 -c "SELECT id || ':' || label FROM lifecycle_fixture ORDER BY id" 2> "$work/transaction.err")" == '1:committed' ]]
[[ "$(psql -XAtw -v ON_ERROR_STOP=1 -c "SELECT datlocprovider::text || ':' || datcollate || ':' || datctype FROM pg_database WHERE datname = current_database()" 2> "$work/locale.err")" == 'c:en_US.utf8:en_US.utf8' ]]
printf 'healthy_after_negatives=pass\n'
