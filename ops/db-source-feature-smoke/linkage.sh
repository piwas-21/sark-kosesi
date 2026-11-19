#!/usr/bin/env bash
# Accept ldd output on stdin. A missing library is a failure, not an absence.
set -euo pipefail
report=$(cat)
[[ -n "$report" ]] || exit 1
if grep -Eqi 'not found|lib(xml2|xslt|gssapi|krb5|ldap|lber)[^[:space:]/]*[.]' <<<"$report"; then
  exit 1
fi
for required in 'libicu' 'libssl' 'libcrypto' 'libz[.]'; do
  grep -Eqi "$required" <<<"$report" || exit 1
done
