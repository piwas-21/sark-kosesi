#!/usr/bin/env bash
set -euo pipefail
source ops/db-source-feature-smoke/diagnostics.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
printf '%s\n' 'psql: error: connection to server failed: FATAL:  password authentication failed for user "postgres"' > "$work/auth"
printf '%s\n' 'ERROR:  0A000: unsupported XML feature' > "$work/xml"
printf '%s\n' 'psql: error: connection refused' > "$work/connection"
check_auth_failure 2 "$work/auth"
check_xml_failure 1 "$work/xml"
for code in 0 1 3 127; do if check_auth_failure "$code" "$work/auth"; then exit 1; fi; done
for code in 0 2 3 127; do if check_xml_failure "$code" "$work/xml"; then exit 1; fi; done
if check_auth_failure 2 "$work/connection" || check_xml_failure 1 "$work/connection"; then exit 1; fi
if check_auth_failure 2 "$work/xml" || check_xml_failure 1 "$work/auth"; then exit 1; fi
printf '%s\n' 'diagnostic controls passed'
