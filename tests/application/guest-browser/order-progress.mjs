import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";

export const orderProgressPath =
  /^\/api\/test\/guest\/orders\/[0-9a-f]{32}\/progress$/;

export function allowsBrowserOrderProgress(
  { method, url, site, origin },
  expected,
) {
  return (
    method === "GET" &&
    typeof url === "string" &&
    orderProgressPath.test(url) &&
    site === "same-origin" &&
    (origin === undefined || origin === expected)
  );
}

export async function checkOrderProgress(call, orderId, expected) {
  const path = `/api/test/guest/orders/${orderId}/progress`;
  assert(orderProgressPath.test(path), "exact fixture order identifier");
  const response = await call(path, "GET");
  assert.equal(response.status, 200, "owned order progress");
  assert.equal(response.cache, "private, no-store", "progress cache policy");
  assert(
    isDeepStrictEqual(response.body, expected),
    "closed progress response",
  );
}
