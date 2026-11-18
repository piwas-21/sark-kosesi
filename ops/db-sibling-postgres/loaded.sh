#!/usr/bin/env bash
# Observe only synthetic same-user processes in a network-disabled container.
set -euo pipefail
export PATH=/opt/pg-smoke/bin:$PATH
export LC_ALL=C
work=/tmp/sibling-loaded
mkdir -m 700 "$work"
initdb -D "$work/data" --locale=en_US.utf8 --encoding=UTF8 --auth=trust --username=postgres > /dev/null
pg_ctl -D "$work/data" -l "$work/server.log" -o "-h '' -k $work -p 55433" -w start > /dev/null
client=''
cleanup() {
  if [[ -n "$client" ]]; then kill "$client" 2>/dev/null || true; wait "$client" 2>/dev/null || true; fi
  pg_ctl -D "$work/data" -m fast -w stop > /dev/null
}
trap cleanup EXIT
server=$(head -n 1 "$work/data/postmaster.pid")
[[ "$server" =~ ^[0-9]+$ ]]
psql -X -h "$work" -p 55433 -U postgres -d postgres -v ON_ERROR_STOP=1 -c 'select pg_sleep(20)' >/dev/null 2>&1 &
client=$!
ready=no
for ((n=0; n<100; n++)); do
  if [[ -r "/proc/$client/maps" ]] && grep -q 'libreadline[.]so' "/proc/$client/maps"; then ready=yes; break; fi
  kill -0 "$client"
  sleep 0.1
done
[[ "$ready" == yes ]]
printf 'PROCESS postgres\n'
awk '$6 ~ /^\// {print $6}' "/proc/$server/maps" | LC_ALL=C sort -u
printf 'PROCESS psql\n'
awk '$6 ~ /^\// {print $6}' "/proc/$client/maps" | LC_ALL=C sort -u
