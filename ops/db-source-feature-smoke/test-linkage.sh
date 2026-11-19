#!/usr/bin/env bash
set -euo pipefail
checker=ops/db-source-feature-smoke/linkage.sh
good=$'libicuuc.so => /lib/libicuuc.so\nlibssl.so => /lib/libssl.so\nlibcrypto.so => /lib/libcrypto.so\nlibz.so => /lib/libz.so'
printf '%s\n' "$good" | bash "$checker"
for bad in 'libxml2.so => /lib/libxml2.so' 'libgssapi_krb5.so => /lib/libgssapi_krb5.so' 'libldap.so => /lib/libldap.so' 'libexample.so => not found'; do
  if printf '%s\n%s\n' "$good" "$bad" | bash "$checker"; then exit 1; fi
done
if printf '%s\n' 'libicuuc.so' | bash "$checker"; then exit 1; fi
printf '%s\n' 'linkage controls passed'
