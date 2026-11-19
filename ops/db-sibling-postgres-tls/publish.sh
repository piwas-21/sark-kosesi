#!/usr/bin/env bash

publish_tls_evidence() {
  local staged=$1 destination=$2
  [[ ! -e "$destination" && ! -L "$destination" ]] || return 1
  (
    set -o pipefail
    cd "$staged" || exit 1
    find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum
  ) > "${staged}.SHA256SUMS" || return 1
  mv -- "${staged}.SHA256SUMS" "$staged/SHA256SUMS" || return 1
  mv -- "$staged" "$destination"
}
