#!/usr/bin/env bash
# Exercise transfer controls using separate local directories as simulated namespaces.
set -euo pipefail
source ops/db-source-feature-smoke/transfer.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/source" "$work/target"
docker() {
  [[ "$1" == exec ]] || return 1
  shift
  if [[ "$1" == -i ]]; then shift; fi
  local id=$1
  shift
  if [[ "$1" == sha256sum ]]; then
    sha256sum "$work/$id/${2##*/}"
    return
  fi
  [[ "$1" == sh && "$2" == -c ]] || return 1
  local script=$3
  shift 4
  local path="$work/$id/${1##*/}"
  shift
  sh -c "$script" _ "$path" "$@" || return 1
  if [[ "${CORRUPT_READ:-}" == yes && "$script" == *"cat \"\$1\""* ]]; then printf x; fi
  if [[ "${CORRUPT_TRANSFER:-}" == yes && "$script" == *'cat >'* ]]; then
    printf x >> "$path"
  fi
}
printf '%s\n' 'synthetic archive bytes' > "$work/original"
transfer_in source /tmp/archive "$work/original" 128
transfer_out source /tmp/archive "$work/copy" 128
transfer_in target /tmp/archive "$work/copy" 128
cmp "$work/original" "$work/target/archive"
if transfer_in target /tmp/archive "$work/original" 1; then exit 1; fi
if transfer_out source /tmp/archive "$work/copy" 1; then exit 1; fi
: > "$work/empty"
if transfer_in target /tmp/archive "$work/empty" 128; then exit 1; fi
if transfer_out source /tmp/missing "$work/copy" 128 2>/dev/null; then exit 1; fi
CORRUPT_READ=yes
if transfer_out source /tmp/archive "$work/copy" 128; then exit 1; fi
CORRUPT_READ=no
CORRUPT_TRANSFER=yes
if transfer_in target /tmp/archive "$work/original" 128; then exit 1; fi
printf '%s\n' 'transfer controls passed'
