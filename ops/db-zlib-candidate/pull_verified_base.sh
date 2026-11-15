#!/bin/sh
# Register and verify the immutable base in the disposable runner's image store.
set -eu
base='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
expected='sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d'
if [ "$#" -ne 1 ]; then
  printf '%s\n' 'zlib candidate: invalid base-pull arguments' >&2
  exit 2
fi
log=$1
if docker pull --platform linux/amd64 "$base" > "$log" 2>&1; then
  :
else
  rc=$?
  printf 'zlib candidate: stage=base_pull category=generic_exit exit=%s\n' "$rc" >&2
  exit "$rc"
fi
if image_id=$(docker image inspect "$base" --format '{{.Id}}' 2> "$log"); then
  :
else
  rc=$?
  printf 'zlib candidate: stage=base_inspect category=generic_exit exit=%s\n' "$rc" >&2
  exit "$rc"
fi
if [ "$image_id" != "$expected" ]; then
  printf '%s\n' 'zlib candidate: stage=base_inspect category=identity_mismatch exit=1' >&2
  exit 1
fi
printf '%s\n' "$image_id"
