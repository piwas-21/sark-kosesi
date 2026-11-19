#!/usr/bin/env bash
# Match only expected PostgreSQL/libpq failures; never print diagnostics.
check_failure() {
  local kind=$1 code=$2 file=$3
  [[ "$code" == 2 && -f "$file" ]] || return 1
  case "$kind" in
    wrong_ca) grep -Fq 'SSL error: certificate verify failed' "$file" ;;
    wrong_host) grep -Fq 'server certificate for "sibling-db.test" does not match host name "other-db.test"' "$file" ;;
    no_tls) grep -Fq 'pg_hba.conf rejects connection for host "127.0.0.1", user "postgres", database "postgres", no encryption' "$file" ;;
    wrong_password) grep -Fq 'password authentication failed for user "postgres"' "$file" ;;
    *) return 1 ;;
  esac
}
