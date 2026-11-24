import assert from "node:assert/strict";

export const pendingCancellationPath =
  /^\/api\/test\/guest\/orders\/[0-9a-f]{32}\/pending-cancellation$/;

export function allowsPendingCancellationRead(
  { method, url, site, origin },
  expected,
) {
  return (
    method === "GET" &&
    typeof url === "string" &&
    pendingCancellationPath.test(url) &&
    site === "same-origin" &&
    (origin === undefined || origin === expected)
  );
}

export async function exercisePendingCancellation({
  call,
  otherCall,
  reload,
  csrf,
  quoteInput,
  order,
  stageDispatch,
}) {
  const quote = await call(
    "/api/guest/nl/quotes",
    "POST",
    { ...quoteInput, effect_key: "browser-pending-cancellation-quote" },
    csrf,
  );
  assert.equal(quote.status, 200);
  assert.equal(quote.body.replayed, false);
  const accepted = await call(
    "/api/test/guest/orders",
    "POST",
    {
      ...order,
      quote_id: quote.body.quote.id,
      effect_key: "browser-pending-cancellation-order",
    },
    csrf,
  );
  assert.equal(accepted.status, 200);
  assert.equal(accepted.body.replayed, false);
  const orderId = accepted.body.id;
  assert.match(orderId, /^[a-f0-9]{32}$/);
  const path = `/api/test/guest/orders/${orderId}/pending-cancellation`;
  const intent = {
    effect_key: "browser-pending-cancellation",
    confirmed: true,
  };

  const beforeStart = await call(path, "GET");
  assert.equal(beforeStart.status, 200);
  assert.deepEqual(beforeStart.body, { request: null });
  assert.equal((await call(path, "POST", intent, csrf)).status, 409);

  await stageDispatch(orderId);
  assert.deepEqual((await call(path, "GET")).body, { request: null });
  const foreignGuest = await otherCall("/api/guest/session", "POST", {});
  assert.equal(foreignGuest.status, 200);
  assert.match(foreignGuest.body.csrf_token, /^[a-f0-9]{64}$/);
  assert.equal((await otherCall(path, "GET")).status, 404);
  assert.equal(
    (await otherCall(path, "POST", intent, foreignGuest.body.csrf_token))
      .status,
    404,
  );
  assert.equal((await call(path, "POST", intent, "0".repeat(64))).status, 403);

  const recorded = await call(path, "POST", intent, csrf);
  assert.equal(recorded.status, 200);
  const requestedAt = recorded.body.requested_at;
  assert(Number.isSafeInteger(requestedAt) && requestedAt > 0);
  assert.deepEqual(recorded.body, {
    requested_at: requestedAt,
    replayed: false,
  });
  const replay = await call(path, "POST", intent, csrf);
  assert.equal(replay.status, 200);
  assert.deepEqual(replay.body, { requested_at: requestedAt, replayed: true });

  await reload();
  const refreshed = await call(path, "GET");
  assert.equal(refreshed.status, 200);
  assert.equal(refreshed.cache, "private, no-store");
  assert.deepEqual(refreshed.body, { request: { requested_at: requestedAt } });
  assert.equal((await otherCall(path, "GET")).status, 404);
  return { orderId, requestedAt };
}
