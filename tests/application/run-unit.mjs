import { mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { transform } from "esbuild";
import { MCR } from "monocart-coverage-reports";
import { discoverUnitTests } from "./unit-inventory.mjs";

const sources = [
  "apps/web/src",
  "packages/design/src",
  "packages/room-fit/src",
  "packages/room-draft/src",
];
const coverage = process.argv.slice(2);
if (
  coverage.length > 1 ||
  (coverage.length === 1 && coverage[0] !== "--coverage")
)
  throw new Error("Invalid unit runner arguments");
const files = discoverUnitTests();
const raw = coverage.length
  ? mkdtempSync(join(tmpdir(), "sark-unit-coverage-"))
  : null;
try {
  if (raw) rmSync("coverage/typescript", { recursive: true, force: true });
  let count = 0;
  for (const file of files) {
    const result = spawnSync(
      process.execPath,
      [
        "--experimental-test-module-mocks",
        "--import",
        "tsx",
        "--test",
        "--test-reporter=tap",
        "tests/application/unit-bootstrap.mts",
      ],
      {
        env: {
          ...process.env,
          SARK_UNIT_FILE: file,
          ...(raw ? { NODE_V8_COVERAGE: raw } : {}),
        },
        encoding: "utf8",
        timeout: 60_000,
        maxBuffer: 10 * 1024 * 1024,
      },
    );
    process.stdout.write(result.stdout ?? "");
    process.stderr.write(result.stderr ?? "");
    if (result.error || result.status !== 0)
      throw new Error(`Unit tests failed: ${file}`);
    const total = /^# tests (\d+)$/m.exec(result.stdout)?.[1];
    const passed = /^# pass (\d+)$/m.exec(result.stdout)?.[1];
    const skipped = /^# skipped (\d+)$/m.exec(result.stdout)?.[1];
    const todo = /^# todo (\d+)$/m.exec(result.stdout)?.[1];
    if (
      !total ||
      Number(total) < 1 ||
      total !== passed ||
      skipped !== "0" ||
      todo !== "0"
    )
      throw new Error(`Missing or incomplete unit results: ${file}`);
    count += Number(total);
  }
  console.log(`All ${count} unit cases passed across ${files.length} files.`);
  const controls = spawnSync(
    process.execPath,
    [
      "--test",
      "--test-reporter=tap",
      "tests/application/unit-runtime-controls.test.mjs",
    ],
    {
      encoding: "utf8",
      timeout: 60_000,
      maxBuffer: 10 * 1024 * 1024,
    },
  );
  process.stdout.write(controls.stdout ?? "");
  process.stderr.write(controls.stderr ?? "");
  if (
    controls.error ||
    controls.status !== 0 ||
    !/^# tests ([1-9]\d*)$/m.test(controls.stdout) ||
    /^# tests (\d+)$/m.exec(controls.stdout)?.[1] !==
      /^# pass (\d+)$/m.exec(controls.stdout)?.[1] ||
    !/^# skipped 0$/m.test(controls.stdout) ||
    !/^# todo 0$/m.test(controls.stdout)
  )
    throw new Error("Unit runner controls failed");
  if (raw) {
    const sourceRoots = sources.map((dir) => resolve(dir));
    const included = (path) => {
      const file = path.startsWith("file:")
        ? fileURLToPath(path)
        : resolve(path);
      return sourceRoots.some((root) => file.startsWith(`${root}${sep}`));
    };
    const report = MCR({
      name: "TypeScript application coverage",
      outputDir: "coverage/typescript",
      dataDir: raw,
      reports: ["json-summary", "lcovonly", "console-details"],
      v8Ignore: false,
      entryFilter: (entry) => included(entry.url),
      sourceFilter: included,
      all: {
        dir: sources,
        filter: (path) => /\.(ts|tsx)$/.test(path),
        transformer: async (entry) => {
          const filename = fileURLToPath(entry.url);
          const result = await transform(entry.source, {
            loader: filename.endsWith(".tsx") ? "tsx" : "ts",
            sourcefile: filename,
            sourcemap: "external",
            jsx: "automatic",
          });
          entry.source = result.code;
          entry.sourceMap = JSON.parse(result.map);
        },
      },
    });
    if (!(await report.generate()))
      throw new Error("No TypeScript coverage generated");
    const summary = JSON.parse(
      readFileSync("coverage/typescript/coverage-summary.json", "utf8"),
    ).total;
    for (const [metric, threshold] of Object.entries({
      lines: 85,
      branches: 80,
      functions: 85,
      statements: 85,
    })) {
      const item = summary[metric];
      if (
        !item ||
        !Number.isInteger(item.total) ||
        item.total <= 0 ||
        !Number.isInteger(item.covered) ||
        item.covered * 100 < threshold * item.total
      )
        throw new Error(`${metric} coverage below ${threshold}%`);
    }
    const lcov = resolve("coverage/typescript/lcov.info");
    if (!statSync(lcov).size) throw new Error("Empty LCOV report");
    for (const dir of sources) {
      const critical =
        dir === "apps/web/src"
          ? [
              "apps/web/src/proxy.ts",
              "apps/web/src/lib/published-price.ts",
              "apps/web/src/lib/read-image.ts",
            ]
          : [];
      const args = [
        "scripts/check_app_coverage.py",
        "--report",
        lcov,
        "--package",
        dir,
      ];
      for (const file of critical) args.push("--critical", file);
      const checked = spawnSync("python3", args, { stdio: "inherit" });
      if (checked.error || checked.status !== 0)
        throw new Error(`Application coverage rejected for ${dir}`);
    }
  }
} finally {
  if (raw) rmSync(raw, { recursive: true, force: true });
}
