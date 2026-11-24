import assert from "node:assert/strict";
import { test } from "node:test";
import { customerLogoutRoute } from "./customer-logout.mjs";

test("bound logout allows only DELETE of the exact test session route", () => {
  const [method, pattern] = customerLogoutRoute;
  assert.equal(method, "DELETE");
  assert.equal(pattern.test("/api/test/customer/session"), true);
  for (const wrong of [
    "/api/customer/session",
    "/api/test/customer/session?unrelated=1",
    "/api/test/customer/session/",
    "/api/test/customer/sessi%6fn",
    "/api/test/customer/login-with-csrf",
  ])
    assert.equal(pattern.test(wrong), false);
});
