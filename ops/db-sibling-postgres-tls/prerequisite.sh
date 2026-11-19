#!/usr/bin/env bash
# Only the existing public prerequisite status and build logs are retained.
run_composition() {
  local child=$1 log=$2 destination=$3 name file rc
  if (umask 022; COMPOSE_REVISION="$TLS_REVISION" bash "$child" > "$log" 2>&1); then rc=0; else rc=$?; fi
  for name in db-sibling-postgres-evidence db-sibling-root-evidence db-source-feature-evidence; do
    for file in status.txt build.log; do
      if [[ -f "$RUNNER_TEMP/$name/$file" && ! -L "$RUNNER_TEMP/$name/$file" ]]; then
        # A bounded tail retains the failure at the end of a build log.
        tail -c 1048576 "$RUNNER_TEMP/$name/$file" > "$destination/$name-$file" || return 1
      fi
    done
  done
  return "$rc"
}
