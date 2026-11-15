#!/bin/sh
# Emit only fixed diagnostics from untrusted package-manager output.
set -eu

case "${1:-}" in
  apt_update|apt_cache|apt_download|dpkg_install) stage=$1 ;;
  *) printf '%s\n' 'zlib candidate: invalid diagnostic stage' >&2; exit 2 ;;
esac
case "${2:-}" in
  ''|*[!0-9]*|????*) printf '%s\n' 'zlib candidate: invalid diagnostic status' >&2; exit 2 ;;
  *) status=$2 ;;
esac
if [ "$status" -eq 0 ] || [ "$status" -gt 255 ] || [ "$#" -ne 3 ]; then
  printf '%s\n' 'zlib candidate: invalid diagnostic arguments' >&2
  exit 2
fi
log=$3
category=generic_exit
if grep -Eiq -- 'NO_PUBKEY|invalid signature|signature verification failed|not signed|gpg error|untrusted repository' "$log" 2>/dev/null; then
  category=signature
elif grep -Eiq -- 'certificate|TLS|SSL|CAInfo' "$log" 2>/dev/null; then
  category=tls_certificate
elif grep -Eiq -- 'temporary failure resolving|name or service not known|could not resolve|DNS' "$log" 2>/dev/null; then
  category=dns
elif grep -Eiq -- '(^|[^0-9])(4[0-9][0-9]|5[0-9][0-9])([^0-9]|$)|HTTP error|http response' "$log" 2>/dev/null; then
  category=http
elif grep -Eiq -- 'unable to locate package|version .* was not found|no installation candidate' "$log" 2>/dev/null; then
  category=package_unavailable
fi
printf 'zlib candidate: stage=%s category=%s exit=%s\n' "$stage" "$category" "$status" >&2
