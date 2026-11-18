#!/bin/sh
# Invoked inside an owned, network-disabled image.
set -eu
case "$1" in
  packages) dpkg-query -W -f='${binary:Package}\t${Version}\n' | LC_ALL=C sort ;;
  loader) ldconfig -p ;;
  libc) sha256sum /lib/x86_64-linux-gnu/libc.so.6 | cut -d ' ' -f 1 ;;
  locale)
    locale -a | LC_ALL=C sort
    LANG=en_US.utf8 LC_ALL=en_US.utf8 locale
    printf '%s\n' 'A-C' 'AB' 'café' 'مرحبا' | LC_ALL=en_US.utf8 sort
    ;;
  no-postgres)
    command -v sh >/dev/null
    if command -v postgres >/dev/null 2>&1; then exit 1; fi
    test ! -e /opt/pg-smoke/bin/postgres
    ;;
  *) exit 1 ;;
esac
