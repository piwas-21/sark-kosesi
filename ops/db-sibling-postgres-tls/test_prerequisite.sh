#!/usr/bin/env bash
set -euo pipefail
source ops/db-sibling-postgres-tls/prerequisite.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export RUNNER_TEMP="$work/runner" TLS_REVISION=synthetic
mkdir "$RUNNER_TEMP" "$work/evidence"
cat > "$work/child" <<'CHILD'
#!/usr/bin/env bash
set -euo pipefail
[[ "$(umask)" == 0022 ]]
mkdir "$RUNNER_TEMP/db-sibling-root-evidence"
printf 'stage=build outcome=failed exit=17\n' > "$RUNNER_TEMP/db-sibling-root-evidence/status.txt"
python3 -c 'print("x" * 1048576)' > "$RUNNER_TEMP/db-sibling-root-evidence/build.log"
printf 'public build failure\n' >> "$RUNNER_TEMP/db-sibling-root-evidence/build.log"
printf 'not public\n' > "$RUNNER_TEMP/db-sibling-root-evidence/private.txt"
mkdir "$RUNNER_TEMP/db-source-feature-evidence"
ln -s "$RUNNER_TEMP/db-sibling-root-evidence/private.txt" "$RUNNER_TEMP/db-source-feature-evidence/build.log"
printf 'not public either\n' >&2
exit "${CHILD_EXIT:-17}"
CHILD
umask 077
rc=0
run_composition "$work/child" "$work/raw.log" "$work/evidence" || rc=$?
[[ "$rc" == 17 && "$(umask)" == 0077 ]]
grep -Fxq 'stage=build outcome=failed exit=17' "$work/evidence/db-sibling-root-evidence-status.txt"
grep -Fxq 'public build failure' "$work/evidence/db-sibling-root-evidence-build.log"
[[ "$(wc -c < "$work/evidence/db-sibling-root-evidence-build.log" | tr -d ' ')" == 1048576 ]]
[[ "$(find "$work/evidence" -type f | wc -l | tr -d ' ')" == 2 ]]
[[ -s "$work/raw.log" ]]
export RUNNER_TEMP="$work/runner-success" CHILD_EXIT=0
mkdir "$RUNNER_TEMP" "$work/evidence-success"
run_composition "$work/child" "$work/raw-success.log" "$work/evidence-success"
[[ "$(umask)" == 0077 ]]
grep -Fxq 'public build failure' "$work/evidence-success/db-sibling-root-evidence-build.log"
printf '%s\n' 'Prerequisite isolation and evidence controls passed'
