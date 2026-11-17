import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import * as esm from "minimatch";

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

test("removed TypeScript ESLint parser is not installed", () => {
  assert.throws(
    () => createRequire(import.meta.url).resolve("@typescript-eslint/parser"),
    {
      code: "MODULE_NOT_FOUND",
    },
  );
});
