import assert from "node:assert/strict";
import test from "node:test";

import { stagingWebHealthy } from "../../deploy/staging/web-health.mjs";

const home = "http://127.0.0.1:3000/nl";
const collection = `${home}/collection`;
const policy = `${home}/winkelinformatie`;
const admin = `${home}/admin`;
const api = "http://127.0.0.1:45671/health/ready";

function responses(entries) {
  const expected = new Map(entries);
  return async (url, options) => {
    assert.equal(options.redirect, "manual");
    assert.ok(options.signal instanceof AbortSignal);
    assert.ok(expected.has(url));
    const status = expected.get(url);
    expected.delete(url);
    return new Response(status === 204 ? null : "test response", { status });
  };
}

test("private and public previews retain their separate storefront expectations", async () => {
  for (const [env, status] of [
    [{}, 401],
    [{ STAGING_PUBLIC_STOREFRONT: "true" }, 200],
  ]) {
    const request = responses([
      [home, status],
      [admin, 401],
      [api, 204],
    ]);
    assert.equal(await stagingWebHealthy(env, request), true);
  }
});

test("Dutch review health requires available catalogue and policies, denied admin, and API readiness", async () => {
  const env = {
    STAGING_PUBLIC_STOREFRONT: "true",
    STAGING_STORE_REVIEW_CONTENT: "{}",
  };
  const healthy = {
    [collection]: 200,
    [policy]: 200,
    [admin]: 404,
    [api]: 204,
  };
  assert.equal(
    await stagingWebHealthy(env, responses(Object.entries(healthy))),
    true,
  );
  for (const [path, status] of [
    [collection, 404],
    [collection, 307],
    [policy, 404],
    [admin, 401],
    [api, 200],
    [api, 503],
  ]) {
    const request = responses(Object.entries({ ...healthy, [path]: status }));
    assert.equal(await stagingWebHealthy(env, request), false, path);
  }
  const disconnected = () => Promise.reject(new Error("connection failed"));
  assert.equal(await stagingWebHealthy(env, disconnected), false);
});
