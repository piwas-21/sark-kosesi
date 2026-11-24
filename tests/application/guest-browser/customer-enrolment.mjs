import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";
import { customerListPaths } from "./customer-lists.mjs";

const email = "verify@example.test";
const password = "synthetic verification fixture passphrase";
const customerCookie = "__Host-piwas-customer";
const redeemPath = "/api/test/verification/redeem";
const loginPath = "/api/test/customer/login-with-csrf";
const historyPath = "/api/test/customer/linked-orders";
const bearer = /^[a-f0-9]{64}$/;

export const customerEnrolmentRoute = [
  "POST",
  /^\/api\/test\/verification\/redeem$/,
];

function expectResponse(response, status, label, body) {
  assert.equal(response.status, status, `${label} status`);
  assert.equal(response.cache, "private, no-store", `${label} cache policy`);
  if (body !== undefined)
    assert(isDeepStrictEqual(response.body, body), `${label} closed body`);
}

async function protectedReads(call, status) {
  const history = await call(historyPath, "GET");
  expectResponse(
    history,
    status,
    "enrolment linked-order history",
    status === 200 ? { orders: [], has_more: false } : undefined,
  );
  for (const path of customerListPaths) {
    expectResponse(
      await call(path, "GET"),
      status,
      "enrolment customer list",
      status === 200 ? { revision: "0", items: [] } : undefined,
    );
  }
}

export async function checkCustomerEnrolment({
  call,
  context,
  origin,
  proof,
  verifyPristine,
  completePassword,
  verifyCustody,
}) {
  assert(bearer.test(proof), "one exact fixture mail proof is required");
  const cookies = () => context.cookies(origin);
  const noCookie = async () => {
    assert(
      (await cookies()).length === 0,
      "enrolment issued an unexpected cookie",
    );
  };
  const redeem = (mailbox, token) =>
    call(redeemPath, "POST", { email: mailbox, token });
  const login = () => call(loginPath, "POST", { email, password });
  const denied = { code: "verification_denied" };

  await noCookie();
  await protectedReads(call, 401);
  const wrong = (proof[0] === "0" ? "1" : "0") + proof.slice(1);
  expectResponse(await redeem(email, wrong), 401, "wrong proof", denied);
  expectResponse(
    await redeem("unrelated-enrolment@example.test", proof),
    401,
    "wrong mailbox",
    denied,
  );
  await noCookie();
  await verifyPristine();

  expectResponse(await redeem(email, proof), 204, "exact verification", null);
  expectResponse(
    await redeem(email, proof),
    401,
    "verification replay",
    denied,
  );
  await noCookie();
  await protectedReads(call, 401);
  expectResponse(await login(), 401, "login before password setup", {
    code: "login_denied",
  });
  await noCookie();
  await completePassword();
  await noCookie();
  await protectedReads(call, 401);

  const loggedIn = await login();
  expectResponse(loggedIn, 200, "enrolled customer login");
  assert(
    loggedIn.body &&
      Object.keys(loggedIn.body).length === 1 &&
      typeof loggedIn.body.csrf_token === "string" &&
      bearer.test(loggedIn.body.csrf_token),
    "login must return exactly one bound synchronizer",
  );
  const sessionCookies = await cookies();
  assert(sessionCookies.length === 1, "expected one enrolled customer cookie");
  const [session] = sessionCookies;
  assert(
    session.name === customerCookie &&
      session.secure &&
      session.httpOnly &&
      session.sameSite === "Lax" &&
      session.path === "/" &&
      session.domain === new URL(origin).hostname &&
      bearer.test(session.value),
    "expected one secure host-only customer cookie",
  );
  await protectedReads(call, 200);
  await verifyCustody();
}
