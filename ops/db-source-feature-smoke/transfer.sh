#!/usr/bin/env bash
# Namespace-local, bounded file transfers for owned synthetic fixtures only.
transfer_in() {
  local id=$1 path=$2 file=$3 limit=$4 size local_hash remote_hash
  [[ -s "$file" ]] || return 1
  size=$(wc -c < "$file") || return 1
  [[ "$size" -le "$limit" ]] || return 1
  local_hash=$(sha256sum "$file" | cut -d " " -f 1) || return 1
  docker exec -i "$id" sh -c 'set -eu; umask 077; cat > "$1"; test -s "$1"; test -r "$1"' _ "$path" < "$file" || return 1
  remote_hash=$(docker exec "$id" sha256sum "$path" | cut -d " " -f 1) || return 1
  [[ "$local_hash" == "$remote_hash" ]]
}

transfer_out() {
  local id=$1 path=$2 file=$3 limit=$4 remote_hash local_hash
  remote_hash=$(docker exec "$id" sha256sum "$path" | cut -d " " -f 1) || return 1
  docker exec "$id" sh -c 'set -eu; test -s "$1"; test "$(wc -c < "$1")" -le "$2"; cat "$1"' _ "$path" "$limit" > "$file" || return 1
  [[ -s "$file" ]] || return 1
  local_hash=$(sha256sum "$file" | cut -d " " -f 1) || return 1
  [[ "$remote_hash" == "$local_hash" ]]
}
