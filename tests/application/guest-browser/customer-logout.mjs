import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";
import {
  checkNewCustomerListProof,
  customerListPaths,
} from "./customer-lists.mjs";

const sessionPath = "/api/test/customer/session";
const historyPath = "/api/test/customer/linked-orders";
const protectedPaths = [historyPath, ...customerListPaths];
const customerCookie = "__Host-piwas-customer";
const guestCookie = "__Host-piwas-guest";
export const customerLogoutRoute = [
  "DELETE",
  /^\/api\/test\/customer\/session$/,
];

function status(response, expected, label) {
  assert.equal(response.status, expected, `${label} status`);
  assert.equal(response.cache, "private, no-store", `${label} cache policy`);
}

async function cookie(context, origin, name) {
  const values = (await context.cookies(origin)).filter(
    (entry) => entry.name === name,
  );
  assert(values.length <= 1, "fixture cookie must be unambiguous");
  return values[0];
}

function secureSession(value, origin) {
  assert(
    value &&
      value.secure &&
      value.httpOnly &&
      value.sameSite === "Lax" &&
      value.path === "/" &&
      value.domain === new URL(origin).hostname &&
      /^[a-f0-9]{64}$/.test(value.value),
    "expected an exact secure host-only customer session",
  );
}

async function readProtected(call, expected) {
  const views = [];
  for (const path of protectedPaths) {
    const response = await call(path, "GET");
    status(response, expected, "protected customer read");
    views.push(response.body);
  }
  return views;
}

function unchanged(current, expected, label) {
  assert(isDeepStrictEqual(current, expected), label);
}

function logout(call, proof) {
  return call(sessionPath, "DELETE", undefined, proof);
}

// `openOwnerTab` opens a second page in the already authenticated owner context
// and returns its fetch helper plus a close callback. `loginOwner` reuses the
// existing test login transport, with no credentials stored by this scenario.
export async function checkCustomerLogout({
  origin,
  owner,
  other,
  ownerContext,
  otherContext,
  ownerCsrf,
  otherCsrf,
  ownedOrderId,
  openOwnerTab,
  loginOwner,
}) {
  const original = await cookie(ownerContext, origin, customerCookie);
  const unrelated = await cookie(otherContext, origin, customerCookie);
  const guest = await cookie(ownerContext, origin, guestCookie);
  secureSession(original, origin);
  secureSession(unrelated, origin);
  assert(guest, "fixture retains the independent guest cookie");
  const tab = await openOwnerTab();
  try {
    const ownerViews = await readProtected(tab.call, 200);
    assert(
      ownerViews[0]?.orders?.length === 1 &&
        ownerViews[0].orders[0]?.order_id === ownedOrderId,
      "the already-open tab must start with the exact owner's linked order",
    );
    unchanged(
      ownerViews.slice(1),
      [
        { revision: "2", items: [] },
        { revision: "2", items: [] },
      ],
      "owner cleared lists before logout",
    );
    const otherViews = await readProtected(other, 200);
    unchanged(otherViews[0]?.orders, [], "second account has no linked orders");
    unchanged(
      otherViews.slice(1),
      [
        { revision: "0", items: [] },
        { revision: "0", items: [] },
      ],
      "second account never changed its lists",
    );

    status(
      await logout(owner, otherCsrf),
      401,
      "foreign proof in owner context",
    );
    status(
      await logout(other, ownerCsrf),
      401,
      "foreign proof in second context",
    );
    assert(
      (await cookie(ownerContext, origin, customerCookie))?.value ===
        original.value,
      "foreign proof changed the owner's cookie",
    );
    assert(
      (await cookie(otherContext, origin, customerCookie))?.value ===
        unrelated.value,
      "foreign proof changed the second cookie",
    );
    unchanged(
      await readProtected(tab.call, 200),
      ownerViews,
      "denial preserved owner access",
    );
    unchanged(
      await readProtected(other, 200),
      otherViews,
      "denial preserved second-account access",
    );

    const closed = await logout(owner, ownerCsrf);
    status(closed, 204, "committed logout");
    assert(closed.body === null, "logout response must have an empty body");
    assert(
      (await cookie(ownerContext, origin, customerCookie)) === undefined,
      "logout retained the customer cookie",
    );
    assert(
      (await cookie(ownerContext, origin, guestCookie))?.value === guest.value,
      "logout altered the independent guest cookie",
    );
    await readProtected(tab.call, 401);
    status(
      await logout(owner, ownerCsrf),
      401,
      "repeated logout after cookie removal",
    );
    unchanged(
      await readProtected(other, 200),
      otherViews,
      "owner logout changed second-account ownership",
    );

    const renewed = await loginOwner();
    status(renewed, 200, "clean owner login");
    const proof = renewed.body?.csrf_token;
    assert(
      renewed.body &&
        Object.keys(renewed.body).length === 1 &&
        typeof proof === "string" &&
        /^[a-f0-9]{64}$/.test(proof) &&
        proof !== ownerCsrf,
      "clean login must return one newly bound proof",
    );
    const replacement = await cookie(ownerContext, origin, customerCookie);
    secureSession(replacement, origin);
    assert(
      replacement.value !== original.value,
      "clean login reused the original cookie",
    );
    unchanged(
      await readProtected(tab.call, 200),
      ownerViews,
      "clean login did not restore the same owned history and lists",
    );
    status(await logout(owner, ownerCsrf), 401, "old proof on the new session");
    assert(
      (await cookie(ownerContext, origin, customerCookie))?.value ===
        replacement.value,
      "old proof removed the new session",
    );
    await checkNewCustomerListProof(tab.call, proof);
    unchanged(
      await readProtected(tab.call, 200),
      ownerViews,
      "new session lost owner access",
    );
    assert(
      (await cookie(otherContext, origin, customerCookie))?.value ===
        unrelated.value,
      "owner session changes affected the second cookie",
    );
    unchanged(
      await readProtected(other, 200),
      otherViews,
      "owner re-login changed second-account ownership",
    );
  } finally {
    await tab.close();
  }
}
