// Standalone Next.js review-mode acceptance with a disposable synthetic upstream.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { once } from "node:events";
import { access, cp, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import {
  reviewContent,
  reviewEnvironment,
} from "../../../apps/web/tests/store-review-fixture.ts";
import { stagingWebHealthy } from "../../../deploy/staging/web-health.mjs";
import { availablePort } from "../ports.mjs";
import { runStoreReviewSmoke } from "../store-review-smoke.mjs";
import { checkBrowser } from "./browser.mjs";
import { covers, foam, image, startUpstream } from "./upstream.mjs";

const web = fileURLToPath(new URL("../../../apps/web/", import.meta.url));
const built = join(web, ".next");
const standalone = join(built, "standalone");
const entry = "apps/web/server.js";
const token = randomBytes(32).toString("hex");
const revision = randomBytes(20).toString("hex");

async function waitForWeb(origin, child) {
  for (let attempt = 0; attempt < 60; attempt++) {
    if (child.exitCode !== null || child.signalCode !== null) break;
    try {
      const response = await fetch(`${origin}/robots.txt`, {
        signal: AbortSignal.timeout(500),
      });
      await response.body?.cancel();
      if (
        response.ok &&
        response.headers.get("x-piwas-preview-revision") === revision
      )
        return;
    } catch {
      // The owned loopback process may still be starting.
    }
    await delay(250);
  }
  throw new Error("The owned standalone storefront did not become ready");
}

async function checkReadback(environment, connectionOrigin) {
  const origin = environment.CANONICAL_ORIGIN;
  const imageSha256 = createHash("sha256").update(image).digest("hex");
  const products = [covers, foam].map((product) => ({
    slug: product.family_slug,
    sku: product.sku,
    title: product.copy.title,
    price: product.price_decimal,
    fill: product.fill,
    imagePath: `/catalogue-images/nl/${product.sku}/1/320`,
    imageSha256,
  }));
  assert.deepEqual(
    await runStoreReviewSmoke({
      origin,
      connectionOrigin,
      revision,
      evidence: { store: reviewContent, products },
    }),
    {
      origin,
      revision,
      transport: "private-loopback",
      approvedProducts: 2,
      collectionPages: 1,
    },
  );
}

async function checkReviewDenials(origin, adminToken) {
  const authorization = `Basic ${Buffer.from(`test-admin:${adminToken}`).toString("base64")}`;
  for (const [method, path] of [
    ["GET", "/nl/admin"],
    ["POST", "/nl/collection"],
    ["GET", "/nl/collection/isolated-room?variant=SET-MISSING"],
  ]) {
    const response = await fetch(`${origin}${path}`, {
      method,
      headers: { authorization },
      redirect: "manual",
      signal: AbortSignal.timeout(5000),
    });
    await response.body?.cancel();
    assert.equal(response.status, 404, `${method} ${path}`);
    assert.equal(response.headers.get("x-piwas-preview-revision"), revision);
    assert.equal(response.headers.get("www-authenticate"), null);
  }
}

async function checkHealth(environment, origin, upstream) {
  const endpoint = new Map([
    ["http://127.0.0.1:3000", origin],
    ["http://127.0.0.1:45671", upstream.origin],
  ]);
  const request = (input, options) => {
    const url = new URL(input);
    const target = endpoint.get(url.origin);
    assert.ok(target, "Only owned loopback probe destinations are allowed");
    return fetch(`${target}${url.pathname}${url.search}`, options);
  };
  assert.equal(await stagingWebHealthy(environment, request), true);
  upstream.setState("empty");
  try {
    assert.equal(await stagingWebHealthy(environment, request), false);
  } finally {
    upstream.setState("ready");
  }
  assert.equal(await stagingWebHealthy(environment, request), true);
}

try {
  await access(join(standalone, entry));
} catch {
  throw new Error(
    "Run npm run build --workspace @piwas/web before the storefront rehearsal",
  );
}
const temporary = await mkdtemp(join(tmpdir(), "sark-store-review-"));
let upstream;
let child;
let closed;
try {
  await cp(standalone, temporary, { recursive: true, verbatimSymlinks: true });
  await Promise.all([
    cp(join(built, "static"), join(temporary, "apps/web/.next/static"), {
      recursive: true,
    }),
    cp(join(web, "public"), join(temporary, "apps/web/public"), {
      recursive: true,
    }),
  ]);
  upstream = await startUpstream(token);
  const port = await availablePort();
  const origin = `http://127.0.0.1:${port}`;
  const environment = {
    ...reviewEnvironment,
    API_URL: upstream.origin,
    PREVIEW_TOKEN: token,
    PREVIEW_REVISION: revision,
    STAGING_ADMIN_TOKEN: randomBytes(32).toString("hex"),
    HOSTNAME: "127.0.0.1",
    PORT: String(port),
    NEXT_TELEMETRY_DISABLED: "1",
  };
  child = spawn(process.execPath, [join(temporary, entry)], {
    env: environment,
    stdio: "inherit",
  });
  closed = new Promise((resolve) => child.once("close", resolve));
  await once(child, "spawn");
  await waitForWeb(origin, child);
  await checkReadback(environment, origin);
  await checkReviewDenials(origin, environment.STAGING_ADMIN_TOKEN);
  await checkHealth(environment, origin, upstream);
  await checkBrowser(origin, upstream.setState);
  console.log(
    "Dutch storefront runtime passed HTTP, health, desktop/mobile and recovery checks with synthetic data.",
  );
} finally {
  if (child) {
    child.kill("SIGTERM");
    const force = setTimeout(() => child.kill("SIGKILL"), 5000);
    try {
      await closed;
    } finally {
      clearTimeout(force);
    }
  }
  try {
    await upstream?.close();
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}
