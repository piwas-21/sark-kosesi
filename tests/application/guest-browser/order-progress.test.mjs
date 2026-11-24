import assert from "node:assert/strict";
import { test } from "node:test";
import { allowsBrowserOrderProgress } from "./order-progress.mjs";

test("the progress Origin bridge only serves an exact same-origin GET", () => {
  const expected = "https://127.0.0.1:12000";
  const order = "ab".repeat(16);
  const url = `/api/test/guest/orders/${order}/progress`;
  const request = { method: "GET", url, site: "same-origin" };
  assert.equal(allowsBrowserOrderProgress(request, expected), true);
  assert.equal(
    allowsBrowserOrderProgress({ ...request, origin: expected }, expected),
    true,
  );
  for (const change of [
    { method: "POST" },
    { url: `${url}?cursor=other` },
    { url: url.replace(order, order.toUpperCase()) },
    { url: url.replace(order, "%61" + order.slice(1)) },
    { url: url.replace("/progress", "/pending-cancellation") },
    { site: "same-site" },
    { site: "cross-site" },
    { site: undefined },
    { origin: "https://127.0.0.1:12001" },
    { origin: [expected, expected] },
  ]) {
    assert.equal(
      allowsBrowserOrderProgress({ ...request, ...change }, expected),
      false,
    );
  }
});
