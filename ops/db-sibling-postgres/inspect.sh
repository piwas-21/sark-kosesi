#!/usr/bin/env bash
# Inspect only the owned PostgreSQL installation inside an offline container.
set -euo pipefail
export LC_ALL=C
case "$1" in
  files)
    test -d /opt/pg-smoke/bin
    find /opt/pg-smoke -type f -print0 | sort -z | xargs -0 sha256sum
    ;;
  metadata)
    find /opt/pg-smoke -printf '%m %U %G %p %l\n' | sort
    ;;
  ownership)
    test -z "$(find /opt/pg-smoke \( -nouser -o -nogroup -o ! -user root -o ! -group root \) -print -quit)"
    test -z "$(find /opt/pg-smoke ! -type l -perm /022 -print -quit)"
    ;;
  linkage)
    count=0
    find /opt/pg-smoke -type f \( -perm /111 -o -name '*.so*' \) -print0 > /tmp/sibling-postgres-files.unsorted
    sort -z /tmp/sibling-postgres-files.unsorted > /tmp/sibling-postgres-files.sorted
    while IFS= read -r -d '' file; do
      magic=$(od -An -tx1 -N4 "$file" | tr -d ' \n')
      if [[ "$magic" != 7f454c46 ]]; then continue; fi
      printf 'FILE %s\n' "$file"
      if ldd "$file" > /tmp/sibling-postgres-ldd.txt 2>&1; then
        cat /tmp/sibling-postgres-ldd.txt
      elif grep -Eq '^[[:space:]]*statically linked[[:space:]]*$' /tmp/sibling-postgres-ldd.txt; then
        cat /tmp/sibling-postgres-ldd.txt
      else
        cat /tmp/sibling-postgres-ldd.txt >&2
        exit 1
      fi
      count=$((count + 1))
    done < /tmp/sibling-postgres-files.sorted
    [[ "$count" -gt 0 ]]
    ;;
  *) exit 1 ;;
esac
