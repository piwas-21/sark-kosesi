// Synthetic private browser transport exercise. Never targets a deployed service.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer, request as httpsRequest } from "node:https";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { chromium } from "@playwright/test";
import { availablePort } from "../ports.mjs";

if (process.env.APP_ENV !== "test") throw new Error("Requires test mode");
if (process.env.PGOPTIONS !== undefined)
  throw new Error("Invalid disposable PostgreSQL configuration");
const rawDatabase = process.env.DATABASE_URL ?? "";
let database;
try {
  database = new URL(rawDatabase);
} catch {
  throw new Error("Invalid disposable PostgreSQL configuration");
}
if (
  !/^postgresql:\/\/p1_test:[A-Za-z0-9_-]{43}@127\.0\.0\.1:[1-9][0-9]{0,4}\/postgres$/.test(
    rawDatabase,
  ) ||
  database.port === "0" ||
  Number(database.port) > 65535
)
  throw new Error("Invalid disposable PostgreSQL configuration");
const directory = await mkdtemp(join(tmpdir(), "sark-guest-browser-"));
const token = randomBytes(32).toString("hex");
const disposableMarker = process.env.P1_POSTGRES_DISPOSABLE_MARKER ?? "";
if (!/^[0-9a-f]{64}$/.test(disposableMarker))
  throw new Error("Requires the disposable cluster marker");
const clusterUrl = database.href;
const fixtureName = `sark_guest_browser_${randomBytes(16).toString("hex")}`;
const fixtureMarker = randomBytes(32).toString("hex");
database.pathname = `/${fixtureName}`;
const apiPort = await availablePort();
const api = `http://127.0.0.1:${apiPort}`;
const children = [];
const run = (command, args, env = process.env) =>
  new Promise((resolveRun, reject) => {
    const child = spawn(command, args, { env, stdio: "inherit" });
    children.push(child);
    child.once("error", reject);
    child.once("exit", (code) =>
      code === 0
        ? resolveRun()
        : reject(new Error(`${command} failed (${code})`)),
    );
  });
const address = {
  recipient: "Synthetic Recipient",
  line1: "Synthetic Street 22",
  line2: null,
  city: "Fixture City",
  region: null,
  country: "NL",
  postcode: "1234 AB",
};
const now = Math.floor(Date.now() / 1000);
const seller = {
  id: "SYNTHETIC-SELLER",
  revision: 1,
  locale: "nl",
  legal_name: "Piwas synthetic test seller",
  registration_id: "synthetic-registration-not-real",
  tax_registration: {
    kind: "vat_registered",
    vat_id: "synthetic-tax-id-not-real",
  },
  address,
  support_email: "support@example.test",
  terms_version: "synthetic-terms-v1",
  terms_sha256: "a".repeat(64),
  privacy_version: "synthetic-privacy-v1",
  privacy_sha256: "b".repeat(64),
  valid_from: 1000,
  expires_at: now + 3600,
  evidence_ref: "synthetic seller/legal/tax evidence only",
};
const routes = [
  ["POST", /^\/api\/guest\/session$/],
  ["PUT", /^\/api\/guest\/basket$/],
  ["POST", /^\/api\/guest\/nl\/quotes$/],
  ["POST", /^\/api\/test\/guest\/orders$/],
  ["POST", /^\/api\/test\/guest\/orders\/[0-9a-f]{32}\/cancellation$/],
];
let origin = "";
let server;
let browser;
let crossOrigin;
try {
  const cert = spawnSync(
    "openssl",
    [
      "req",
      "-x509",
      "-newkey",
      "rsa:2048",
      "-nodes",
      "-keyout",
      join(directory, "key.pem"),
      "-out",
      join(directory, "cert.pem"),
      "-days",
      "1",
      "-subj",
      "/CN=127.0.0.1",
      "-addext",
      "subjectAltName=IP:127.0.0.1",
    ],
    { stdio: "ignore" },
  );
  if (cert.status !== 0)
    throw new Error("Could not create ephemeral fixture certificate");
  server = createServer(
    {
      key: await readFile(join(directory, "key.pem")),
      cert: await readFile(join(directory, "cert.pem")),
    },
    async (request, response) => {
      const deny = (status) => {
        response.writeHead(status, { "cache-control": "no-store" });
        response.end();
      };
      if (
        request.headers.host !== new URL(origin).host ||
        request.headers.authorization
      )
        return deny(403);
      if (request.method === "GET" && request.url === "/") {
        response.writeHead(200, {
          "content-type": "text/html; charset=utf-8",
          "cache-control": "no-store",
          "content-security-policy":
            "default-src 'self'; connect-src 'self'; frame-ancestors 'none'",
        });
        response.end(
          '<!doctype html><html lang="en"><title>Synthetic private browser fixture</title><main>Private test fixture</main></html>',
        );
        return;
      }
      if (
        request.headers.origin !== origin ||
        !routes.some(
          ([method, pattern]) =>
            method === request.method && pattern.test(request.url ?? ""),
        )
      )
        return deny(403);
      const chunks = [];
      let size = 0;
      for await (const chunk of request) {
        size += chunk.length;
        if (size > 4096) return deny(413);
        chunks.push(chunk);
      }
      const headers = { authorization: `Bearer ${token}`, origin };
      for (const name of ["content-type", "cookie", "x-csrf-token"])
        if (request.headers[name]) headers[name] = request.headers[name];
      try {
        const upstream = await fetch(`${api}${request.url}`, {
          method: request.method,
          headers,
          body: Buffer.concat(chunks),
          redirect: "error",
          signal: AbortSignal.timeout(5000),
        });
        const result = Buffer.from(await upstream.arrayBuffer());
        if (result.includes(token)) return deny(502);
        const outgoing = {
          "cache-control": "private, no-store",
          "content-type":
            upstream.headers.get("content-type") ?? "application/json",
        };
        const cookies = upstream.headers.getSetCookie();
        if (cookies.length) outgoing["set-cookie"] = cookies;
        response.writeHead(upstream.status, outgoing);
        response.end(result);
      } catch {
        deny(502);
      }
    },
  );
  await new Promise((ok, fail) => {
    server.once("error", fail);
    server.listen(0, "127.0.0.1", ok);
  });
  const bound = server.address();
  if (!bound || typeof bound === "string") throw new Error("No fixture port");
  origin = `https://127.0.0.1:${bound.port}`;
  const env = {
    ...process.env,
    APP_ENV: "test",
    DATABASE_URL: database.href,
    GUEST_BROWSER_CLUSTER_URL: clusterUrl,
    GUEST_BROWSER_DATABASE_NAME: fixtureName,
    GUEST_BROWSER_DATABASE_MARKER: fixtureMarker,
    PREVIEW_TOKEN: token,
    API_BIND: `127.0.0.1:${apiPort}`,
    GUEST_ORIGIN: origin,
    TEST_GUEST_ORDER_TRANSPORT: "1",
    TEST_GUEST_CANCELLATION_TRANSPORT: "1",
    TEST_GUEST_ORDER_SELLER: JSON.stringify(seller),
  };
  delete env.CALLBACK_INTAKE_PROFILE_ID;
  for (const key of Object.keys(env))
    if (
      key.startsWith("TEST_") &&
      ![
        "TEST_GUEST_ORDER_TRANSPORT",
        "TEST_GUEST_CANCELLATION_TRANSPORT",
        "TEST_GUEST_ORDER_SELLER",
      ].includes(key)
    )
      delete env[key];
  const testArgs = [
    "test",
    "--locked",
    "-p",
    "commerce-infrastructure",
    "--test",
    "guest_browser_fixture",
    "--",
    "--ignored",
    "--exact",
  ];
  await run(
    "cargo",
    [...testArgs, "verify_guest_browser_database_absent"],
    env,
  );
  const wrongMarker = disposableMarker.startsWith("0")
    ? "1".repeat(64)
    : "0".repeat(64);
  const denied = spawnSync(
    "cargo",
    [...testArgs, "seed_guest_browser_database"],
    {
      env: { ...env, P1_POSTGRES_DISPOSABLE_MARKER: wrongMarker },
      encoding: "utf8",
      maxBuffer: 1_000_000,
    },
  );
  if (
    denied.status === 0 ||
    !`${denied.stdout ?? ""}${denied.stderr ?? ""}`.includes(
      "cluster ownership marker mismatch",
    )
  )
    throw new Error("Foreign cluster marker negative control failed");
  await run(
    "cargo",
    [...testArgs, "verify_guest_browser_database_absent"],
    env,
  );
  await run("cargo", [...testArgs, "seed_guest_browser_database"], env);
  await run("cargo", ["build", "--locked", "-p", "commerce-api"], env);
  const binary = resolve(
    process.env.CARGO_TARGET_DIR ?? "target",
    "debug/commerce-api",
  );
  const apiEnv = { ...env };
  delete apiEnv.GUEST_BROWSER_CLUSTER_URL;
  delete apiEnv.GUEST_BROWSER_DATABASE_MARKER;
  delete apiEnv.GUEST_BROWSER_DATABASE_NAME;
  delete apiEnv.P1_POSTGRES_DISPOSABLE_MARKER;
  const child = spawn(binary, [], { env: apiEnv, stdio: "inherit" });
  children.push(child);
  let ready = false;
  for (let i = 0; i < 80; i++) {
    try {
      if (
        (
          await fetch(`${api}/health/ready`, {
            signal: AbortSignal.timeout(500),
          })
        ).ok
      ) {
        const denied = await fetch(`${api}/api/catalogue/nl`, {
          signal: AbortSignal.timeout(500),
        });
        const accepted = await fetch(`${api}/api/catalogue/nl`, {
          headers: { authorization: `Bearer ${token}` },
          signal: AbortSignal.timeout(500),
        });
        ready =
          denied.status === 401 &&
          accepted.status === 200 &&
          child.exitCode === null;
        if (ready) break;
      }
    } catch {
      /* starting */
    }
    if (child.exitCode !== null) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  if (!ready || child.exitCode !== null)
    throw new Error("Owned API did not start");
  assert.equal((await fetch(`${api}/api/guest/basket`)).status, 401);
  const wrongOriginStatus = await new Promise((resolveStatus, reject) => {
    const request = httpsRequest(
      `${origin}/api/guest/session`,
      {
        method: "POST",
        rejectUnauthorized: false,
        headers: {
          origin: "https://wrong.invalid",
          "content-type": "application/json",
        },
      },
      (response) => {
        response.resume();
        resolveStatus(response.statusCode);
      },
    );
    request.on("error", reject);
    request.end("{}");
  });
  assert.equal(wrongOriginStatus, 403);
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ ignoreHTTPSErrors: true });
  const allowedOrigins = new Set([origin]);
  let blockedNetwork = false;
  await context.route("**/*", (route) => {
    if (allowedOrigins.has(new URL(route.request().url()).origin))
      return route.continue();
    blockedNetwork = true;
    return route.abort();
  });
  const page = await context.newPage();
  const outbound = [];
  page.on("request", (request) => outbound.push(request));
  const fixture = await page.goto(origin);
  assert.equal(fixture.status(), 200);
  assert(!(await fixture.text()).includes(token));
  const call = (path, method, data, csrf) =>
    page.evaluate(
      async ({ path, method, data, csrf }) => {
        const headers = { "content-type": "application/json" };
        if (csrf) headers["x-csrf-token"] = csrf;
        const response = await fetch(path, {
          method,
          headers,
          body: JSON.stringify(data),
          credentials: "same-origin",
        });
        return {
          status: response.status,
          cache: response.headers.get("cache-control"),
          body: await response.json(),
        };
      },
      { path, method, data, csrf },
    );
  const guest = await call("/api/guest/session", "POST", {}, null);
  assert.equal(guest.status, 200);
  assert.equal(guest.body.revision, 0);
  const csrf = guest.body.csrf_token;
  assert.match(csrf, /^[a-f0-9]{64}$/);
  assert.equal(await page.evaluate(() => document.cookie), "");
  const cookies = await context.cookies(origin);
  assert.equal(cookies.length, 1);
  assert.equal(cookies[0].name, "__Host-piwas-guest");
  assert.equal(cookies[0].secure, true);
  assert.equal(cookies[0].httpOnly, true);
  assert.equal(cookies[0].sameSite, "Lax");
  // The same cookie-bearing browser context cannot mutate across origins.
  crossOrigin = createServer(
    {
      key: await readFile(join(directory, "key.pem")),
      cert: await readFile(join(directory, "cert.pem")),
    },
    (_request, response) => {
      response.writeHead(200, { "content-type": "text/html" });
      response.end("<!doctype html><title>Cross-origin fixture</title>");
    },
  );
  await new Promise((ok, fail) => {
    crossOrigin.once("error", fail);
    crossOrigin.listen(0, "127.0.0.1", ok);
  });
  const crossBound = crossOrigin.address();
  if (!crossBound || typeof crossBound === "string")
    throw new Error("No cross-origin port");
  const foreignOrigin = `https://127.0.0.1:${crossBound.port}`;
  allowedOrigins.add(foreignOrigin);
  const attacker = await context.newPage();
  await attacker.goto(foreignOrigin);
  const crossed = await attacker.evaluate(
    async ({ origin, csrf }) => {
      try {
        await fetch(`${origin}/api/guest/basket`, {
          method: "PUT",
          credentials: "include",
          headers: { "content-type": "application/json", "x-csrf-token": csrf },
          body: "{}",
        });
        return true;
      } catch {
        return false;
      }
    },
    { origin, csrf },
  );
  assert.equal(crossed, false);
  await attacker.close();
  const basket = {
    effect_key: "browser-basket",
    expected_revision: 0,
    lines: [{ sku: "SET-FILLED", quantity: 3 }],
  };
  assert.equal(
    (await call("/api/guest/basket", "PUT", basket, "0".repeat(64))).status,
    403,
  );
  assert.equal(
    (await call("/api/guest/basket", "PUT", basket, csrf)).body.revision,
    1,
  );
  const quoteInput = {
    expected_basket_revision: 1,
    effect_key: "browser-quote",
    country: "NL",
    postcode: "1234 AB",
  };
  const quote = await call("/api/guest/nl/quotes", "POST", quoteInput, csrf);
  const quoteReplay = await call(
    "/api/guest/nl/quotes",
    "POST",
    quoteInput,
    csrf,
  );
  assert.equal(quote.status, 200);
  assert.equal(quote.body.replayed, false);
  assert.equal(quoteReplay.body.replayed, true);
  assert.equal(quoteReplay.body.quote.id, quote.body.quote.id);
  assert.equal(quote.body.quote.total.gross_minor, 38245);
  assert.equal(quote.body.quote.indicative, true);
  const order = {
    effect_key: "browser-order",
    quote_id: quote.body.quote.id,
    expected_basket_revision: 1,
    buyer: { name: "Synthetic Buyer", email: "buyer@example.test" },
    shipping_address: address,
    billing_address: address,
    accepted_terms_version: "synthetic-terms-v1",
    confirmed: true,
  };
  const stranger = await browser.newContext({ ignoreHTTPSErrors: true });
  const strangerPage = await stranger.newPage();
  await strangerPage.goto(origin);
  const missingCookie = await strangerPage.evaluate(
    async ({ order, csrf }) => {
      const response = await fetch("/api/test/guest/orders", {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": csrf },
        body: JSON.stringify(order),
      });
      return response.status;
    },
    { order, csrf },
  );
  assert.equal(missingCookie, 401);
  await stranger.close();
  const first = await call("/api/test/guest/orders", "POST", order, csrf);
  const again = await call("/api/test/guest/orders", "POST", order, csrf);
  assert.equal(first.status, 200);
  assert.equal(first.body.replayed, false);
  assert.equal(again.body.replayed, true);
  assert.equal(first.body.id, again.body.id);
  assert.match(first.body.id, /^[a-f0-9]{32}$/);
  const cancellationPath = `/api/test/guest/orders/${first.body.id}/cancellation`;
  const cancellation = { effect_key: "browser-cancel", confirmed: true };
  assert.deepEqual(
    (await call(cancellationPath, "POST", cancellation, csrf)).body,
    { replayed: false },
  );
  assert.deepEqual(
    (await call(cancellationPath, "POST", cancellation, csrf)).body,
    { replayed: true },
  );
  assert.equal(first.cache, "private, no-store");
  assert.equal(blockedNetwork, false);
  for (const request of outbound) {
    assert(
      request.url().startsWith(`${origin}/`),
      "Unexpected browser network target",
    );
    assert(
      !Object.hasOwn(request.headers(), "authorization"),
      "API authority reached the browser",
    );
    assert(!request.url().includes(token));
  }
  await context.close();
  await run(
    "cargo",
    [
      "test",
      "--locked",
      "-p",
      "commerce-infrastructure",
      "--test",
      "guest_browser_fixture",
      "--",
      "--ignored",
      "--exact",
      "verify_guest_browser_effects",
    ],
    env,
  );
  console.log(
    "Synthetic Chromium guest quote, acceptance and cancellation transport passed; no provider or public UI acceptance.",
  );
} finally {
  if (browser) await browser.close();
  if (crossOrigin) await new Promise((done) => crossOrigin.close(done));
  if (server) await new Promise((done) => server.close(done));
  for (const child of children.reverse())
    if (child.exitCode === null) child.kill("SIGTERM");
  await rm(directory, { recursive: true, force: true });
}
