#!/usr/bin/env bash
# Runs only inside a disposable container as its non-root postgres user.
set -euo pipefail
bin=$1
role=$2
export PATH="$bin:$PATH"
work=/tmp/feature-smoke
mkdir -m 700 "$work"
printf '%s\n' 'synthetic-feature-password' > "$work/password"
chmod 600 "$work/password"
initdb -D "$work/data" --locale=en_US.utf8 --encoding=UTF8 --auth-local=trust --auth-host=scram-sha-256 --username=postgres --pwfile="$work/password" > /dev/null
pg_ctl -D "$work/data" -l "$work/server.log" -o '-h 127.0.0.1 -p 55432' -w start > /dev/null
trap 'pg_ctl -D "$work/data" -m fast -w stop > /dev/null' EXIT
export PGHOST=127.0.0.1 PGPORT=55432 PGUSER=postgres PGDATABASE=postgres PGPASSWORD=synthetic-feature-password
[[ "$(psql -XAt -v ON_ERROR_STOP=1 -c 'select 42')" == 42 ]]
if PGPASSWORD=wrong-synthetic-password psql -XAt -v ON_ERROR_STOP=1 -c 'select 42' >/dev/null 2>&1; then exit 1; fi
if [[ "$role" == source ]]; then
  [[ "$(psql -XAt -v ON_ERROR_STOP=1 -c "SELECT xmlparse(document '<a/>')::text")" == '<a/>' ]]
  psql -X -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
CREATE TABLE feature_fixture (id integer PRIMARY KEY, value text NOT NULL);
INSERT INTO feature_fixture VALUES (1, repeat('compressible-', 2000)), (2, 'café مرحبا');
SQL
  pg_dump -Fc -Z gzip:9 -f /tmp/feature-smoke.dump
  [[ -s /tmp/feature-smoke.dump ]]
  chmod 644 /tmp/feature-smoke.dump
elif [[ "$role" == target ]]; then
  if psql -XAt -v ON_ERROR_STOP=1 -c "SELECT xmlparse(document '<a/>')" >/dev/null 2>&1; then exit 1; fi
  pg_restore --exit-on-error --dbname postgres /tmp/feature-smoke.dump
  [[ "$(psql -XAt -v ON_ERROR_STOP=1 -c "SELECT count(*) = 2 AND sum(length(value)) = 26010 AND bool_or(value = 'café مرحبا') FROM feature_fixture")" == t ]]
  if printf 'invalid archive' | pg_restore --list >/dev/null 2>&1; then exit 1; fi
else
  exit 1
fi
printf 'role=%s scram_positive=pass scram_negative=pass archive=pass\n' "$role"
