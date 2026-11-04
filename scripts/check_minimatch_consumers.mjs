#!/usr/bin/env node
// Verify each locked minimatch 10 consumer resolves the exact patched package.
import { createRequire } from "node:module";
import { readFileSync, realpathSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const project = resolve(
  process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), ".."),
);
const lock = JSON.parse(
  readFileSync(join(project, "package-lock.json"), "utf8"),
);
const expectedConsumers = [
  "node_modules/@eslint/config-array",
  "node_modules/@typescript-eslint/typescript-estree",
  "node_modules/eslint",
];
const installedPaths = Object.keys(lock.packages).filter((name) =>
  /(?:^|\/)node_modules\/minimatch$/.test(name),
);
if (
  installedPaths.length !== 1 ||
  installedPaths[0] !== "node_modules/minimatch" ||
  lock.packages[installedPaths[0]].version !== "10.2.6"
) {
  throw new Error("Unexpected locked minimatch installation");
}
const consumers = Object.entries(lock.packages)
  .filter(([, info]) => info.dependencies?.minimatch)
  .map(([name]) => name);
if (
  consumers.length !== expectedConsumers.length ||
  !expectedConsumers.every((consumer) => consumers.includes(consumer))
) {
  throw new Error(
    "Unexpected minimatch 10 consumer graph; review backport coverage",
  );
}
const expected = realpathSync(
  join(project, "node_modules/minimatch/dist/commonjs/index.js"),
);
for (const consumer of expectedConsumers) {
  const metadata = join(project, consumer, "package.json");
  const pkg = JSON.parse(readFileSync(metadata, "utf8"));
  if (
    pkg.dependencies?.minimatch !==
    lock.packages[consumer].dependencies.minimatch
  ) {
    throw new Error(`Unexpected installed minimatch dependency: ${consumer}`);
  }
  const actual = realpathSync(createRequire(metadata).resolve("minimatch"));
  if (actual !== expected)
    throw new Error(`Unexpected minimatch resolution: ${consumer}`);
}
console.log("All locked minimatch 10 consumers resolve the verified package");
