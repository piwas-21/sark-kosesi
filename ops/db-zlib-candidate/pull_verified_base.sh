#!/bin/sh
# Pull and verify a pinned database or scanner image on the disposable runner.
set -eu
invalid() {
  printf '%s\n' 'verified mirror image: invalid pull arguments' >&2
  exit 2
}
valid_digest() {
  [ "${#1}" -eq 71 ] && printf '%s\n' "$1" | LC_ALL=C grep -Eq '^sha256:[0-9a-f]{64}$'
}
case "$#" in
  1)
    image='mirror.gcr.io/library/postgres@sha256:c7fedce47eff6a78cb35dad6b627f0d684107f75ab15c0870e40c093766ba452'
    expected='sha256:3cccc725163a95947178f4da5e9bf16add75cb286170ac5a2cd2f6513534955d'
    log=$1
    ;;
  4)
    [ "$1" = '--image' ] || invalid
    image=$2 expected=$3 log=$4
    ;;
  *) invalid ;;
esac
case "$image" in
  mirror.gcr.io/library/postgres@*|mirror.gcr.io/aquasec/trivy@*) ;;
  *) invalid ;;
esac
valid_digest "${image#*@}" && valid_digest "$expected" || invalid

attempt=1
while :; do
  if docker pull --platform linux/amd64 "$image" > "$log" 2>&1; then
    break
  else
    rc=$?
  fi
  # Match explicit registry quota diagnostics without publishing raw pull output.
  if ! LC_ALL=C grep -Eiq '(^|[^[:alnum:]_])toomanyrequests:|(^|[^[:digit:]])429[[:space:]]+too many requests|status( code)?:?[[:space:]]+429([^[:digit:]]|$)' "$log"; then
    printf 'verified mirror image: stage=pull category=generic_exit attempt=%s exit=%s\n' "$attempt" "$rc" >&2
    exit "$rc"
  fi
  printf 'verified mirror image: stage=pull category=mirror_rate_limit attempt=%s exit=%s\n' "$attempt" "$rc" >&2
  case "$attempt" in
    1) sleep 15 ;;
    2) sleep 30 ;;
    *) exit "$rc" ;;
  esac
  attempt=$((attempt + 1))
done

if image_id=$(docker image inspect "$image" --format '{{.Id}}' 2>> "$log"); then
  :
else
  rc=$?
  printf 'verified mirror image: stage=inspect category=generic_exit exit=%s\n' "$rc" >&2
  exit "$rc"
fi
if [ "$image_id" != "$expected" ]; then
  printf '%s\n' 'verified mirror image: stage=inspect category=identity_mismatch exit=1' >&2
  exit 1
fi
printf 'verified mirror image: stage=inspect category=success attempt=%s exit=0\n' "$attempt" >&2
printf '%s\n' "$image_id"
