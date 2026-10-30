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

it("protects all staging response paths and redirects only to the configured origin", () => {
  vi.stubEnv("APP_ENV", "staging");
  vi.stubEnv("PREVIEW_TOKEN", "s".repeat(32));
  vi.stubEnv("PREVIEW_REVISION", "b".repeat(40));
  vi.stubEnv("API_URL", "http://127.0.0.1:45671");
  vi.stubEnv("CANONICAL_ORIGIN", "https://sark-kosesi-test.piwas.nl");
  const authorization = `Basic ${Buffer.from(`preview:${"s".repeat(32)}`).toString("base64")}`;
  for (const [path, supplied, status] of [
    ["/nl", "bad", 401],
    ["/", authorization, 307],
    ["/nl", authorization, 200],
  ] as const) {
    const response = proxy(
      request(path, {
        authorization: supplied,
        host: "forged.invalid",
        "x-forwarded-host": "forged.invalid",
        "x-forwarded-proto": "http",
      }),
    );
    expect(response.status).toBe(status);
    expect(response.headers.get("X-Piwas-Preview-Revision")).toBe(
      "b".repeat(40),
    );
    expect(response.headers.get("Cache-Control")).toBe("private, no-store");
    expect(response.headers.get("X-Robots-Tag")).toBe("noindex, nofollow");
    expect(response.headers.get("Referrer-Policy")).toBe("no-referrer");
    expect(response.headers.get("X-Content-Type-Options")).toBe("nosniff");
    if (path === "/")
      expect(response.headers.get("Location")).toBe(
        "https://sark-kosesi-test.piwas.nl/nl",
      );
  }
  vi.unstubAllEnvs();
});
