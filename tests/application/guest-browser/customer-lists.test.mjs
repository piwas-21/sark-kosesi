import assert from "node:assert/strict";
import { test } from "node:test";
import {
  allowsBrowserCustomerList,
  customerListRoutes,
} from "./customer-lists.mjs";

const sameOrigin = "https://127.0.0.1:41000";
const read = (url) => ({ method: "GET", url, site: "same-origin" });
const route = (method, path) =>
  customerListRoutes.some(
    ([accepted, pattern]) => accepted === method && pattern.test(path),
  );

for (const kind of ["favourites", "restock"]) {
  test(`${kind} read Origin can be supplied only for the exact same-origin fixture`, () => {
    const path = `/api/test/customer/${kind}/nl`;
    const input = read(path);
    assert.equal(allowsBrowserCustomerList(input, sameOrigin), true);
    assert.equal(
      allowsBrowserCustomerList({ ...input, origin: sameOrigin }, sameOrigin),
      true,
    );
    for (const option of [
      { origin: "null" },
      { origin: "https://127.0.0.1:41001" },
      { site: "same-site" },
      { site: "cross-site" },
      { site: undefined },
      { method: "HEAD" },
      { method: "PUT" },
      { url: `${input.url}?fixture=1` },
      { url: `${input.url}/` },
      { url: `${input.url}/SET-FILLED` },
    ])
      assert.equal(
        allowsBrowserCustomerList({ ...input, ...option }, sameOrigin),
        false,
      );
  });

  test(`${kind} fixture bridge only allows its exact Dutch list and fixture SKU mutation`, () => {
    const path = `/api/test/customer/${kind}/nl`;
    const sku = `${path}/SET-FILLED`;
    assert.equal(route("GET", path), true);
    assert.equal(route("PUT", sku), true);
    for (const [method, value] of [
      ["PUT", path],
      ["GET", sku],
      ["DELETE", sku],
      ["GET", `/api/test/customer/${kind}/en`],
      ["PUT", `${path}/SET-COVERS`],
      ["PUT", `${sku}?fixture=1`],
      ["GET", `/api/customer/${kind}/nl`],
    ])
      assert.equal(route(method, value), false);
  });
}
