#!/usr/bin/env bash
# Run only in the verified bootstrap container with its owned disposable volume.
set +x
set -euo pipefail
export LC_ALL=C PATH=/opt/pg-smoke/bin:/usr/bin:/bin
umask 077
work=/tmp/sibling-tls
data=/var/lib/postgresql/18/docker
[[ "$(id -u):$(id -g)" == 65532:65532 ]] || exit 1
[[ -d "$work" && ! -L "$work" ]] || exit 1
[[ "$(stat -c '%u:%g:%a' "$work")" == '65532:65532:700' ]] || exit 1
[[ -d "$data" && ! -L "$data" ]]
[[ "$(stat -c '%u:%g:%a' "$data")" == '65532:65532:700' ]]
[[ -z "$(ls -A "$data")" ]]
for input in password server.crt server.key; do
  file="$work/$input"
  [[ -f "$file" && ! -L "$file" && "$(stat -c '%u:%g:%a' "$file")" == '65532:65532:600' ]]
  [[ "$(wc -c < "$file")" -le 16384 ]]
done
[[ "$(wc -c < "$work/password")" -eq 65 ]]
grep -qxE '[a-f0-9]{64}' "$work/password"
initdb -D "$data" --locale-provider=libc --locale=en_US.utf8 --encoding=UTF8 \
  --auth-local=scram-sha-256 --auth-host=scram-sha-256 --username=postgres \
  --pwfile="$work/password" > "$work/init.log" 2>&1
install -m 600 "$work/server.key" "$data/server.key"
install -m 600 "$work/server.crt" "$data/server.crt"
cat > "$data/pg_hba.conf" <<'HBA'
local all all scram-sha-256
hostssl all all 127.0.0.1/32 scram-sha-256
hostnossl all all 127.0.0.1/32 reject
HBA
printf 'synthetic_bootstrap=pass\n'
