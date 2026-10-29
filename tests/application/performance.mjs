import assert from "node:assert/strict";
import os from "node:os";
import { createHash } from "node:crypto";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { chromium } from "@playwright/test";
import lighthouse from "lighthouse";
import { gzipSync } from "node:zlib";
import { spawnSync } from "node:child_process";
import { availablePort } from "./ports.mjs";
const debuggingPort = await availablePort();
const budgets = JSON.parse(
  await readFile("config/performance-budgets.json", "utf8"),
);
const base = process.env.WEB_BASE_URL;
if (!base || !process.env.PREVIEW_TOKEN)
  throw new Error("Owned app fixture required");
const authorization = `Basic ${Buffer.from(`preview:${process.env.PREVIEW_TOKEN}`).toString("base64")}`;
const database = JSON.parse(
  await readFile(".local/evidence/postgres-fixture.json", "utf8"),
);
assert.equal(database.catalogueItems, 5);
assert.equal(database.translations, 7);
assert.ok(database.databaseBytes > 0);
const evidence = [];
await mkdir(".local/evidence", { recursive: true });
const browser = await chromium.launch({
  headless: true,
  args: [`--remote-debugging-port=${debuggingPort}`],
});
try {
  for (const path of [
    "/nl",
    "/nl/products/courtyard-demo",
    "/nl/gallery",
    "/nl/collection",
    "/nl/collection/isolated-room?variant=SET-COVERS",
  ]) {
    const reports = [];
    for (let run = 0; run < budgets.lab.runs; run++) {
      const report = await lighthouse(base + path, {
        port: debuggingPort,
        output: "json",
        logLevel: "error",
        onlyCategories: ["performance"],
        extraHeaders: { Authorization: authorization },
      });
      assert.ok(report?.lhr);
      reports.push(report.lhr);
      const diagnostics = {
        path,
        run,
        performance: 100 * (report.lhr.categories.performance?.score ?? 0),
        lcpMs: report.lhr.audits["largest-contentful-paint"]?.numericValue,
        tbtMs: report.lhr.audits["total-blocking-time"]?.numericValue,
        cls: report.lhr.audits["cumulative-layout-shift"]?.numericValue,
        fcpMs: report.lhr.audits["first-contentful-paint"]?.numericValue,
        lcpBreakdown: report.lhr.audits["lcp-breakdown-insight"]?.details,
        lcpDiscovery: report.lhr.audits["lcp-discovery-insight"]?.details,
        warnings: report.lhr.runWarnings,
      };
      await writeFile(
        `.local/evidence/lighthouse-${path.replaceAll("/", "_")}-${run}.json`,
        JSON.stringify(diagnostics, null, 2),
      );
    }
    const median = (values) =>
      values.sort((a, b) => a - b)[Math.floor(values.length / 2)];
    const performance = median(
      reports.map(
        (report) => 100 * (report.categories.performance?.score ?? 0),
      ),
    );
    const metric = (id) =>
      median(
        reports.map((report) => report.audits[id]?.numericValue ?? Infinity),
      );
    console.log(
      JSON.stringify({
        path,
        performance,
        lcpMs: metric("largest-contentful-paint"),
        tbtMs: metric("total-blocking-time"),
        cls: metric("cumulative-layout-shift"),
      }),
    );
    assert.ok(
      performance >= budgets.lab.lighthousePerformance,
      `${path} performance ${performance}`,
    );
    assert.ok(
      metric("largest-contentful-paint") <= budgets.lab.lcpMs,
      `${path} LCP ${metric("largest-contentful-paint")}ms`,
    );
    assert.ok(
      metric("total-blocking-time") <= budgets.lab.tbtMs,
      `${path} TBT`,
    );
    assert.ok(
      metric("cumulative-layout-shift") <= budgets.lab.cls,
      `${path} CLS`,
    );
    const context = await browser.newContext({
      httpCredentials: {
        username: "preview",
        password: process.env.PREVIEW_TOKEN,
      },
      viewport: { width: 390, height: 844 },
    });
    const page = await context.newPage();
    const resources = [];
    page.on("response", (response) => {
      if (response.url().startsWith(base) && response.status() === 200)
        resources.push(response);
    });
    await page.goto(base + path, { waitUntil: "networkidle" });
    let total = 0,
      javascript = 0,
      fonts = 0;
    for (const response of resources) {
      const measured = await fetch(response.url(), {
        headers: { Authorization: authorization },
      });
      assert.equal(
        measured.status,
        200,
        "Payload must be fetchable with fixture credentials",
      );
      const body = Buffer.from(await measured.arrayBuffer());
      const bytes = gzipSync(body).length;
      total += bytes;
      const type = response.request().resourceType();
      if (type === "script") javascript += bytes;
      if (type === "font") fonts += bytes;
      if (type === "image")
        assert.ok(
          bytes <= budgets.transfer.mobileLcpImageKiB * 1024,
          "Image budget",
        );
    }
    assert.ok(
      total <= budgets.transfer.initialMobileTotalKiB * 1024,
      `${path} total ${total}`,
    );
    assert.ok(
      javascript <= budgets.transfer.initialFirstPartyJsGzipKiB * 1024,
      `${path} JavaScript ${javascript}`,
    );
    assert.ok(
      fonts <= budgets.transfer.initialFontsKiB * 1024,
      `${path} fonts ${fonts}`,
    );
    evidence.push({
      path,
      performance,
      lcpMs: metric("largest-contentful-paint"),
      tbtMs: metric("total-blocking-time"),
      cls: metric("cumulative-layout-shift"),
      totalGzip: total,
      javascriptGzip: javascript,
      fontsGzip: fonts,
    });
    await context.close();
  }
} finally {
  await browser.close();
}
await mkdir(".local/evidence", { recursive: true });
const result = spawnSync(
  process.env.K6_BINARY ?? "k6",
  [
    "run",
    "--quiet",
    "--summary-export",
    ".local/evidence/load.json",
    "tests/application/load.js",
  ],
  { env: process.env, stdio: "inherit" },
);
if (result.error) throw result.error;
assert.equal(result.status, 0, "k6 isolated catalogue load failed");
await mkdir(".local/evidence", { recursive: true });
await writeFile(
  ".local/evidence/performance.json",
  JSON.stringify(
    {
      scope: "P1 preview production build",
      database,
      revision: process.env.ANALYSIS_REVISION ?? "uncommitted-local-run",
      hardware: {
        cpu: os.cpus()[0]?.model,
        logicalCpus: os.cpus().length,
        memoryBytes: os.totalmem(),
        platform: os.platform(),
        architecture: os.arch(),
      },
      seedSha256: createHash("sha256")
        .update(await readFile("tests/fixtures/catalogue.sql"))
        .digest("hex"),
      cacheState:
        "Lighthouse storage reset; warmed SQL catalogue; private no-store HTTP",
      fieldCoreWebVitals: "not-measured",
      pages: evidence,
    },
    null,
    2,
  ),
);
