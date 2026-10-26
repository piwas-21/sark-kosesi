import { it, expect, vi } from "vitest";
import { NextRequest } from "next/server";
import { proxy } from "../src/proxy";
function request(path: string, headers: Record<string, string> = {}) {
  return new NextRequest(`http://127.0.0.1:3000${path}`, { headers });
}
it("protects previews and isolates locale negotiation and host metadata", () => {
  const token = "a".repeat(32);
  vi.stubEnv("APP_ENV", "test");
  vi.stubEnv("PREVIEW_TOKEN", token);
  vi.stubEnv("API_URL", "http://127.0.0.1:4000");
  vi.stubEnv("CANONICAL_ORIGIN", "http://127.0.0.1:3000");
  expect(proxy(request("/nl")).status).toBe(401);
  expect(proxy(request("/nl", { authorization: "bad" })).status).toBe(401);
  const authorization = `Basic ${Buffer.from(`preview:${token}`).toString("base64")}`;
  expect(
    proxy(
      request("/nl", { authorization: authorization.replace("YWFh", "YmJi") }),
    ).status,
  ).toBe(401);
  expect(proxy(request("/", { authorization })).headers.get("location")).toBe(
    "http://127.0.0.1:3000/nl",
  );
  const redirected = proxy(
    request("/?variant=covers", {
      authorization,
      "accept-language": "ar",
      host: "evil.test",
    }),
  );
  expect(redirected.headers.get("location")).toBe(
    "http://127.0.0.1:3000/ar?variant=covers",
  );
  expect(redirected.headers.get("vary")).toBe("Cookie, Accept-Language");
  const saved = proxy(
    request("/", {
      authorization,
      cookie: "locale=de",
      "accept-language": "ar",
    }),
  );
  expect(saved.headers.get("location")).toBe("http://127.0.0.1:3000/de");
  const explicit = proxy(
    request("/nl", { authorization, "accept-language": "ar" }),
  );
  expect(explicit.headers.get("location")).toBeNull();
  expect(explicit.headers.get("content-security-policy")).toContain(
    "frame-ancestors 'none'",
  );
  expect(explicit.headers.get("x-robots-tag")).toBe("noindex, nofollow");
});
