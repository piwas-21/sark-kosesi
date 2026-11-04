#!/usr/bin/env node
// Apply and verify the bounded development-only backport before loading ESLint.
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
for (const [command, args] of [
  ["python3", [join(root, "scripts/patch_minimatch_10.py"), "--apply"]],
  [
    process.execPath,
    [join(root, "scripts/check_minimatch_consumers.mjs"), root],
  ],
  [process.execPath, [join(root, "node_modules/eslint/bin/eslint.js"), "."]],
]) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
