#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=ops/db-sibling-postgres-tls/publish.sh
source ops/db-sibling-postgres-tls/publish.sh

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/staged" "$work/bin"
printf 'stage=complete outcome=success cleanup=success\n' > "$work/staged/status.txt"
printf 'verify_full=pass\n' > "$work/staged/results.txt"
printf '#!/bin/sh\nexit 1\n' > "$work/bin/sha256sum"
chmod +x "$work/bin/sha256sum"
if PATH="$work/bin:$PATH" publish_tls_evidence "$work/staged" "$work/upload"; then
  exit 1
fi
[[ ! -e "$work/upload" ]]
if (
  mv() {
    [[ ${3:-} != "$work/upload" ]] || return 1
    command mv "$@"
  }
  publish_tls_evidence "$work/staged" "$work/upload"
); then
  exit 1
fi
[[ ! -e "$work/upload" ]]
mkdir "$work/upload"
if publish_tls_evidence "$work/staged" "$work/upload"; then
  exit 1
fi
[[ ! -e "$work/upload/status.txt" ]]
rmdir "$work/upload"
publish_tls_evidence "$work/staged" "$work/upload"
[[ ! -e "$work/staged" ]]
(
  cd "$work/upload"
  sha256sum -c SHA256SUMS
)
