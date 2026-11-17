import { test } from "node:test";
import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { expect, vi } from "./unit-runtime.mjs";

test("assertions reject an incorrect result as well as accepting a match", () => {
  expect(2).toBe(2);
  assert.throws(() => expect(2).toBe(3));
});
test("asynchronous assertions reject an incorrect expected failure", async () => {
  await expect(Promise.reject(new Error("actual"))).rejects.toThrow("actual");
  await assert.rejects(
    expect(Promise.reject(new Error("actual"))).rejects.toThrow("other"),
  );
});
test("DOM and spy matchers detect incorrect expectations", () => {
  const dom = new JSDOM("<p>present</p>");
  const element = dom.window.document.querySelector("p");
  expect(element).toHaveTextContent("present");
  assert.throws(() => expect(element).toHaveTextContent("absent"));
  const call = vi.fn();
  call("actual");
  expect(call).toHaveBeenCalledWith("actual");
  assert.throws(() => expect(call).toHaveBeenCalledWith("other"));
});
test("environment and global stubs restore the original state", () => {
  const key = "PIWAS_UNIT_RUNNER_CONTROL";
  assert.equal(process.env[key], undefined);
  vi.stubEnv(key, "temporary");
  assert.equal(process.env[key], "temporary");
  vi.unstubAllEnvs();
  assert.equal(process.env[key], undefined);
  const original = globalThis.fetch;
  const replacement = () => Promise.reject(new Error("control"));
  vi.stubGlobal("fetch", replacement);
  assert.equal(globalThis.fetch, replacement);
  vi.unstubAllGlobals();
  assert.equal(globalThis.fetch, original);
});

test("coverage maps executed and unexecuted TypeScript sources without hiding branches", async () => {
  const {
    mkdtempSync,
    mkdirSync,
    readFileSync,
    realpathSync,
    rmSync,
    writeFileSync,
  } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const { pathToFileURL, fileURLToPath } = await import("node:url");
  const { transform } = await import("esbuild");
  const { MCR } = await import("monocart-coverage-reports");
  const dir = realpathSync(
    mkdtempSync(join(tmpdir(), "piwas-unit-coverage-control-")),
  );
  try {
    const src = join(dir, "source");
    const raw = join(dir, "raw");
    mkdirSync(src);
    mkdirSync(raw);
    writeFileSync(
      join(src, "covered.tsx"),
      "export function choose(v: boolean) { return v ? 'yes' : 'no'; }\n",
    );
    writeFileSync(
      join(src, "uncovered.ts"),
      "export function never(v: boolean) { return v ? 'a' : 'b'; }\n",
    );
    writeFileSync(
      join(dir, "case.mts"),
      "import {test} from 'node:test'; import assert from 'node:assert/strict'; import {choose} from './source/covered.tsx'; test('executes one branch',()=>assert.equal(choose(true),'yes'));\n",
    );
    const childEnv = { ...process.env, NODE_V8_COVERAGE: raw };
    delete childEnv.NODE_TEST_CONTEXT;
    const child = spawnSync(
      process.execPath,
      ["--import", "tsx", "--test", join(dir, "case.mts")],
      {
        env: childEnv,
        encoding: "utf8",
        timeout: 30_000,
      },
    );
    assert.equal(child.status, 0, child.stderr);
    const output = join(dir, "report");
    const report = MCR({
      name: "Coverage controls",
      baseDir: dir,
      outputDir: output,
      dataDir: raw,
      reports: ["json-summary", "lcovonly"],
      v8Ignore: false,
      entryFilter: (entry) => entry.url.startsWith(pathToFileURL(src).href),
      sourceFilter: (path) => path.includes("source/"),
      all: {
        dir: src,
        filter: (path) => /\.(ts|tsx)$/.test(path),
        transformer: async (entry) => {
          const filename = fileURLToPath(entry.url);
          const result = await transform(entry.source, {
            loader: filename.endsWith(".tsx") ? "tsx" : "ts",
            sourcefile: filename,
            sourcemap: "external",
          });
          entry.source = result.code;
          entry.sourceMap = JSON.parse(result.map);
        },
      },
    });
    assert.ok(await report.generate());
    const summary = JSON.parse(
      readFileSync(join(output, "coverage-summary.json"), "utf8"),
    );
    const covered = Object.entries(summary).find(([name]) =>
      name.endsWith("source/covered.tsx"),
    )?.[1];
    const uncovered = Object.entries(summary).find(([name]) =>
      name.endsWith("source/uncovered.ts"),
    )?.[1];
    assert.ok(covered && uncovered, "both original sources are present");
    assert.ok(covered.lines.covered > 0);
    assert.ok(
      covered.branches.covered > 0 &&
        covered.branches.covered < covered.branches.total,
    );
    assert.equal(uncovered.lines.covered, 0);
    assert.equal(uncovered.functions.covered, 0);
    const lcov = readFileSync(join(output, "lcov.info"), "utf8");
    assert.match(lcov, /SF:source\/covered\.tsx/);
    assert.match(lcov, /SF:source\/uncovered\.ts/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("unit discovery includes nested files and rejects an empty maintained root", async () => {
  const { mkdtempSync, mkdirSync, rmSync, writeFileSync } =
    await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { discoverUnitTests } = await import("./unit-inventory.mjs");
  const dir = mkdtempSync(join(tmpdir(), "piwas-unit-inventory-control-"));
  try {
    mkdirSync(join(dir, "nested"));
    writeFileSync(join(dir, "top.test.tsx"), "");
    writeFileSync(join(dir, "nested", "deeper.test.tsx"), "");
    writeFileSync(join(dir, "nested", "not-included.test.ts"), "");
    const files = discoverUnitTests([{ path: dir, suffix: ".test.tsx" }]);
    assert.deepEqual(files, [
      join(dir, "nested", "deeper.test.tsx"),
      join(dir, "top.test.tsx"),
    ]);
    assert.throws(() =>
      discoverUnitTests([{ path: dir, suffix: ".test.mts" }]),
    );
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
