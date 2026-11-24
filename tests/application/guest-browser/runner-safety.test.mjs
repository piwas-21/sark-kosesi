import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { allowsBrowserHistory } from "./link-policy.mjs";
import { allowsPendingCancellationRead } from "./pending-cancellation.mjs";

const runner = "tests/application/guest-browser/runner.mjs";
function rejected(url, extra = {}) {
  const result = spawnSync(process.execPath, [runner], {
    env: { ...process.env, APP_ENV: "test", DATABASE_URL: url, ...extra },
    encoding: "utf8",
    timeout: 15000,
  });
  assert.notEqual(result.status, 0);
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  assert.match(output, /Invalid disposable PostgreSQL configuration/);
  return output;
}

test("malformed database URL and connect options fail without disclosing input", () => {
  const credential = "synthetic-secret-do-not-print-9e21";
  const endpoint = "private-synthetic-endpoint.invalid";
  const output = rejected(`postgresql://${credential}@${endpoint}:[invalid`);
  assert(!output.includes(credential));
  assert(!output.includes(endpoint));
  const goodShape = `postgresql://p1_test:${"a".repeat(43)}@127.0.0.1:12345/postgres`;
  rejected(`${goodShape}?options=-csark.disposable_marker%3Dforged`);
  rejected(`${goodShape}#fragment`);
  rejected(goodShape, { PGOPTIONS: "-c sark.disposable_marker=forged" });
});

test("only same-origin browser history GET can use the private Origin bridge", () => {
  const expected = "https://127.0.0.1:12345";
  const request = {
    method: "GET",
    url: "/api/test/customer/linked-orders",
    site: "same-origin",
    origin: undefined,
  };
  assert.equal(allowsBrowserHistory(request, expected), true);
  assert.equal(
    allowsBrowserHistory({ ...request, origin: expected }, expected),
    true,
  );
  for (const change of [
    { method: "POST" },
    { url: "/api/test/customer/linked-orders?other=1" },
    { url: "/api/test/customer/guest-order-link/redeem" },
    { site: "same-site" },
    { site: "cross-site" },
    { site: undefined },
    { origin: "https://other.invalid" },
  ])
    assert.equal(
      allowsBrowserHistory({ ...request, ...change }, expected),
      false,
    );
});

test("the pending cancellation Origin bridge requires an exact same-origin order read", () => {
  const expected = "https://127.0.0.1:12345";
  const path = `/api/test/guest/orders/${"a".repeat(32)}/pending-cancellation`;
  const request = { method: "GET", url: path, site: "same-origin" };
  assert.equal(allowsPendingCancellationRead(request, expected), true);
  assert.equal(
    allowsPendingCancellationRead({ ...request, origin: expected }, expected),
    true,
  );
  for (const change of [
    { method: "POST" },
    { url: `${path}?extra=1` },
    { url: path.replace("pending-cancellation", "cancellation") },
    { url: path.replace("a".repeat(32), "a".repeat(31)) },
    { site: "same-site" },
    { site: "cross-site" },
    { site: undefined },
    { origin: "https://other.invalid" },
  ])
    assert.equal(
      allowsPendingCancellationRead({ ...request, ...change }, expected),
      false,
    );
});
