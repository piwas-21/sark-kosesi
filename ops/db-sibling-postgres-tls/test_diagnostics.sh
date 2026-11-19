#!/usr/bin/env bash
set -euo pipefail
source ops/db-sibling-postgres-tls/diagnostics.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf '%s\n' 'psql: error: SSL error: certificate verify failed' > "$work/wrong_ca"
printf '%s\n' 'psql: error: server certificate for "sibling-db.test" does not match host name "other-db.test"' > "$work/wrong_host"
printf '%s\n' 'FATAL:  pg_hba.conf rejects connection for host "127.0.0.1", user "postgres", database "postgres", no encryption' > "$work/no_tls"
printf '%s\n' 'FATAL:  password authentication failed for user "postgres"' > "$work/wrong_password"
printf '%s\n' 'psql: error: connection refused' > "$work/unrelated"
for kind in wrong_ca wrong_host no_tls wrong_password; do
  check_failure "$kind" 2 "$work/$kind"
  for code in 0 1 3 127; do if check_failure "$kind" "$code" "$work/$kind"; then exit 1; fi; done
  if check_failure "$kind" 2 "$work/unrelated"; then exit 1; fi
done
if check_failure wrong_host 2 "$work/wrong_ca" || check_failure no_tls 2 "$work/wrong_password" || check_failure invalid 2 "$work/wrong_ca"; then exit 1; fi
printf '%s\n' 'TLS diagnostic controls passed'
