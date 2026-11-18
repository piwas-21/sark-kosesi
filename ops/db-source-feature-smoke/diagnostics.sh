#!/usr/bin/env bash
# Match failures narrowly; inputs are synthetic diagnostics in a private temp directory.
check_auth_failure() {
  local rc=$1 diagnostic=$2
  [[ "$rc" == 2 ]] && grep -Eq 'FATAL:  password authentication failed for user "postgres"$' "$diagnostic"
}
check_xml_failure() {
  local rc=$1 diagnostic=$2
  [[ "$rc" == 1 ]] && grep -Eq 'ERROR:  0A000: unsupported XML feature$' "$diagnostic"
}
