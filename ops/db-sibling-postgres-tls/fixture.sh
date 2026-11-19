#!/usr/bin/env bash
# Synthetic, disposable in-container TLS test. Raw diagnostics stay in tmpfs.
set -euo pipefail
export PATH=/opt/pg-smoke/bin:$PATH LC_ALL=C
work=/tmp/sibling-tls
# shellcheck source=ops/db-sibling-postgres-tls/diagnostics.sh
source "$work/diagnostics.sh"
[[ "$(id -u)" == 999 && "$(id -g)" == 999 ]]
[[ "$(stat -c '%a %u' "$work/server.key")" == '600 999' ]]
initdb -D "$work/data" --locale=en_US.utf8 --encoding=UTF8 --auth-local=trust --auth-host=scram-sha-256 --username=postgres --pwfile="$work/password" > "$work/init.log" 2>&1
cat > "$work/data/pg_hba.conf" <<'HBA'
local all all trust
hostssl all all 127.0.0.1/32 scram-sha-256
hostnossl all all 127.0.0.1/32 reject
HBA
cat >> "$work/data/postgresql.conf" <<'CONFIG'
ssl = on
ssl_cert_file = '/tmp/sibling-tls/server.crt'
ssl_key_file = '/tmp/sibling-tls/server.key'
CONFIG
pg_ctl -D "$work/data" -l "$work/server.log" -o "-h 127.0.0.1 -p 55434 -k $work" -w start > "$work/start.log" 2>&1
cleanup() {
  rc=$?
  if ! pg_ctl -D "$work/data" -m fast -w stop > "$work/stop.log" 2>&1; then rc=1; fi
  exit "$rc"
}
trap cleanup EXIT
IFS= read -r password < "$work/password"
umask 077
printf '*:*:*:postgres:%s\n' "$password" > "$work/pgpass"
printf '*:*:*:postgres:wrong-%s\n' "$password" > "$work/badpass"
unset password
export PGHOST=sibling-db.test PGHOSTADDR=127.0.0.1 PGPORT=55434 PGUSER=postgres PGDATABASE=postgres
export PGSSLMODE=verify-full PGSSLROOTCERT="$work/ca.crt" PGPASSFILE="$work/pgpass" PGCONNECT_TIMEOUT=3 PGGSSENCMODE=disable
unset PGPASSWORD PGSERVICE PGSERVICEFILE PGOPTIONS
healthy() {
  local result
  result=$(psql -XAtw -v ON_ERROR_STOP=1 -c 'SELECT ssl AND version IS NOT NULL FROM pg_stat_ssl WHERE pid = pg_backend_pid()' 2> "$work/healthy.err")
  [[ "$result" == t ]]
}
healthy
printf 'verify_full=pass\n'
# Confirm the test is exercising a stored SCRAM verifier and the intended HBA rules.
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
printf 'healthy_after_negatives=pass\n'
