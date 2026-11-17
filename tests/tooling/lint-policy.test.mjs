import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const oxlint = join(root, "node_modules/oxlint/bin/oxlint");
const cases = [
  ["ban-ts-comment", '// @ts-ignore\nconst x: number = "a"; console.log(x);'],
  ["no-array-constructor", "const x = new Array(1, 2); console.log(x);"],
  ["no-duplicate-enum-values", "enum X { A=1, B=1 } console.log(X);"],
  ["no-empty-object-type", "interface X {} const x: X = {}; console.log(x);"],
  ["no-explicit-any", "const x: any = 1; console.log(x);"],
  [
    "no-extra-non-null-assertion",
    "declare const x: number | undefined; console.log(x!!);",
  ],
  ["no-misused-new", "interface X { new(): X }"],
  ["no-namespace", "namespace X { export const x = 1; } console.log(X);"],
  [
    "no-non-null-asserted-optional-chain",
    "declare const x: {y:number} | undefined; console.log(x?.y!);",
  ],
  ["no-require-imports", 'const x = require("node:fs"); console.log(x);'],
  [
    "no-this-alias",
    "class X { f() { const self = this; return self; } } console.log(X);",
  ],
  [
    "no-unnecessary-type-constraint",
    "function f<T extends unknown>(x: T) { return x; } console.log(f);",
  ],
  [
    "no-unsafe-declaration-merging",
    "interface X {} class X {} console.log(X);",
  ],
  ["no-unsafe-function-type", "let x: Function; console.log(x);"],
  ["no-unused-expressions", "declare const x: number; x;"],
  ["no-unused-vars", "const unused = 2;"],
  ["no-unused-vars", "const _unused = 2;"],
  [
    "no-unused-vars",
    "export function probe(_unused: string): string { return ''; }",
  ],
  ["no-wrapper-object-types", "let x: String; console.log(x);"],
  ["prefer-as-const", 'let x = "x" as "x"; console.log(x);'],
  [
    "prefer-namespace-keyword",
    "module X { export const y = 1; } console.log(X);",
  ],
  ["triple-slash-reference", '/// <reference path="x" />\nexport {};'],
  [
    "no-non-null-assertion",
    "declare const x: number | undefined; console.log(x!);",
  ],
  ["no-var", "var x = 1; console.log(x);"],
  ["prefer-const", "let x = 1; console.log(x);"],
  ["prefer-rest-params", "function f() { return arguments; } console.log(f);"],
  [
    "prefer-spread",
    "declare const f: (...n: number[]) => void; declare const values: number[]; f.apply(null, values);",
  ],
];

test("each retained TypeScript lint rule detects its own violation", (t) => {
  const dir = mkdtempSync(join(tmpdir(), "sark-lint-policy-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  const file = join(dir, "probe.ts");
  for (const [rule, source] of cases) {
    writeFileSync(file, source);
    const run = spawnSync(
      process.execPath,
      [
        oxlint,
        "--no-ignore",
        "--disable-nested-config",
        "-c",
        join(root, ".oxlintrc.json"),
        "-f",
        "json",
        file,
      ],
      { cwd: root, encoding: "utf8", timeout: 10_000 },
    );
    assert.equal(run.status, 1, `${rule}: ${run.stderr}`);
    const diagnostics = JSON.parse(run.stdout).diagnostics;
    assert.ok(
      diagnostics.some((item) => item.code.endsWith(`(${rule})`)),
      `${rule}: ${run.stdout}`,
    );
  }
  writeFileSync(
    file,
    "export const identity = (value: number): number => value;\n",
  );
  const clean = spawnSync(
    process.execPath,
    [
      oxlint,
      "--no-ignore",
      "--disable-nested-config",
      "-c",
      join(root, ".oxlintrc.json"),
      "-f",
      "json",
      file,
    ],
    { cwd: root, encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(clean.status, 0, clean.stderr);
  assert.deepEqual(JSON.parse(clean.stdout).diagnostics, []);
});

test("lint scope retains source and excludes generated and prototype files", () => {
  const run = spawnSync(
    process.execPath,
    [
      oxlint,
      "--debug",
      "files",
      "--disable-nested-config",
      "-c",
      join(root, ".oxlintrc.json"),
      ".",
    ],
    { cwd: root, encoding: "utf8", timeout: 10_000 },
  );
  assert.equal(run.status, 0, run.stderr);
  const files = new Set(run.stdout.trim().split("\n"));
  for (const source of [
    "apps/web/src/lib/locale.ts",
    "packages/room-fit/src/index.ts",
  ]) {
    assert.ok(files.has(source), `${source} must be linted`);
  }
  for (const excluded of [
    "packages/contracts/generated.ts",
    "packages/contracts/models.gen.ts",
    "apps/web/next-env.d.ts",
    "design/prototype/app.js",
  ]) {
    assert.ok(!files.has(excluded), `${excluded} must remain excluded`);
  }
});
