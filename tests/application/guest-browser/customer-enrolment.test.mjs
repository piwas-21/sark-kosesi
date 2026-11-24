import assert from "node:assert/strict";
import { test } from "node:test";
import { customerEnrolmentRoute } from "./customer-enrolment.mjs";

test("enrolment permits only the exact test verification POST path", () => {
  const [method, pattern] = customerEnrolmentRoute;
  assert.equal(method, "POST");
  assert.equal(pattern.test("/api/test/verification/redeem"), true);
  for (const wrong of [
    "/api/verification/redeem",
    "/api/test/verification",
    "/api/test/verification/redeem?token=unsupported",
    "/api/test/verification/redeem/",
    "/api/test/verification/red%65em",
    "/api/test/customer/login-with-csrf",
  ]) {
    assert.equal(pattern.test(wrong), false);
  }
});
