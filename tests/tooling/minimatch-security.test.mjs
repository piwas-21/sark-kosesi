import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import * as esm from "minimatch";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const commonjs = createRequire(import.meta.url)("minimatch");

for (const [format, api] of [
  ["ESM", esm],
  ["CommonJS", commonjs],
]) {
  test(`${format} bounds expansion and preserves ordinary matching`, () => {
    const pattern = Array(8).fill("**/../a/b").join("/");
    assert.throws(
      () =>
        new api.Minimatch(pattern, {
          optimizationLevel: 2,
          braceExpandMax: 64,
        }),
      /too many patterns/,
    );
    assert.equal(api.minimatch("src/app/index.ts", "src/**/*.ts"), true);
    assert.equal(api.minimatch("src/app/index.js", "src/**/*.ts"), false);
  });

  test(`${format} escapes leading negation and comment characters`, () => {
    for (const name of ["!secret.txt", "#notes.md"]) {
      const pattern = api.escape(name);
      assert.equal(api.minimatch(name, pattern), true);
      assert.equal(api.minimatch("other.txt", pattern), false);
    }
  });
}

test("TypeScript parser project service honors allowDefaultProject globs", (t) => {
  const fixture = mkdtempSync(join(tmpdir(), "sark-parser-minimatch-"));
  t.after(() => rmSync(fixture, { recursive: true, force: true }));
  const file = join(fixture, "probe.js");
  writeFileSync(file, "const answer = 42;\n");
  writeFileSync(
    join(fixture, "tsconfig.json"),
    JSON.stringify({
      compilerOptions: { allowJs: true },
      include: ["src/**/*"],
    }),
  );

  // Each case needs a fresh process because the parser caches its project service.
  const source = `
    import assert from "node:assert/strict";
    import { parseForESLint } from "@typescript-eslint/parser";
    const [file, fixture, glob] = process.argv.slice(1);
    const result = parseForESLint("const answer = 42;", {
      filePath: file,
      tsconfigRootDir: fixture,
      projectService: { allowDefaultProject: [glob] },
    });
    assert.equal(result.ast.body[0].type, "VariableDeclaration");
    assert.ok(result.services.program);
  `;
  const run = (glob) =>
    spawnSync(
      process.execPath,
      ["--input-type=module", "-e", source, file, fixture, glob],
      {
        cwd: root,
        encoding: "utf8",
        timeout: 15_000,
      },
    );
  const allow = run("*.js");
  assert.equal(allow.status, 0, allow.stderr);
  const deny = run("*.ts");
  assert.notEqual(deny.status, 0);
  assert.match(deny.stderr, /not found by the project service/);
});
