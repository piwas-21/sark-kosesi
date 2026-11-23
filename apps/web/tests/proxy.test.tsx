import {
  afterEach,
  it,
  expect,
  vi,
} from "../../../tests/application/unit-runtime.mjs";
import { NextRequest } from "next/server";
import { proxy } from "../src/proxy";
import { configuration } from "../src/lib/config";
import { reviewContent, reviewEnvironment } from "./store-review-fixture";
import { publicStagingRead } from "../src/lib/staging-access";
afterEach(() => vi.unstubAllEnvs());
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

it("opens only storefront reads on opted-in staging and isolates admin credentials", () => {
  vi.stubEnv("APP_ENV", "staging");
  vi.stubEnv("PREVIEW_TOKEN", "s".repeat(32));
  vi.stubEnv("PREVIEW_REVISION", "b".repeat(40));
  vi.stubEnv("API_URL", "http://127.0.0.1:45671");
  vi.stubEnv("CANONICAL_ORIGIN", "https://sark-kosesi-test.piwas.nl");
  vi.stubEnv("STAGING_PUBLIC_STOREFRONT", "true");
  vi.stubEnv("STAGING_ADMIN_TOKEN", "p".repeat(32));
  for (const path of [
    "/nl",
    "/ar/gallery",
    "/nl/collection",
    "/nl/collection/merchant-family",
    "/ar/products/demo-seating",
    "/de/catalogue.xml",
    "/api/catalogue/nl",
    "/catalogue-images/nl/SARK-COVERS/1/640",
    "/robots.txt",
    "/sitemap.xml",
  ]) {
    for (const method of ["GET", "HEAD"]) {
      const response = proxy(
        new NextRequest(`https://sark-kosesi-test.piwas.nl${path}`, { method }),
      );
      expect(response.status).toBe(200);
      expect(response.headers.get("www-authenticate")).toBeNull();
      expect(response.headers.get("x-robots-tag")).toBe("noindex, nofollow");
      expect(response.headers.get("cache-control")).toBe("private, no-store");
      expect(response.headers.get("content-security-policy")).toContain(
        "frame-ancestors 'none'",
      );
    }
  }
  const redirect = proxy(
    request("/?variant=covers", {
      host: "evil.invalid",
      "accept-language": "ar",
    }),
  );
  expect(redirect.status).toBe(307);
  expect(redirect.headers.get("location")).toBe(
    "https://sark-kosesi-test.piwas.nl/ar?variant=covers",
  );
  const authorization = `Basic ${Buffer.from(`test-admin:${"p".repeat(32)}`).toString("base64")}`;
  for (const path of [
    "/nl/admin",
    "/ar/admin/orders",
    "/admin",
    "/api/admin",
    "/api/published/nl",
    "/nl/private",
    "/nl/%61dmin",
    "/nl/collection/private/orders",
  ]) {
    for (const supplied of [
      "",
      "Basic invalid",
      `Basic ${Buffer.from(`preview:${"s".repeat(32)}`).toString("base64")}`,
      authorization.replace("cHBw", "cXFw"),
    ]) {
      const response = proxy(request(path, { authorization: supplied }));
      expect(response.status).toBe(401);
      expect(response.headers.get("www-authenticate")).toContain(
        "Piwas staging admin",
      );
    }
  }
  expect(proxy(request("/nl/admin", { authorization })).status).toBe(200);
  expect(
    proxy(
      new NextRequest("https://sark-kosesi-test.piwas.nl/api/catalogue/nl", {
        method: "POST",
      }),
    ).status,
  ).toBe(401);
  expect(publicStagingRead("/nl", "DELETE")).toBe(false);
  expect(publicStagingRead("/zz", "GET")).toBe(false);
});

it("exposes only Dutch customer reads in store review mode", () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);

  const canonical = proxy(
    request("/?token=example", {
      "accept-language": "ar",
      cookie: "locale=de",
      host: "forged.invalid",
    }),
  );
  expect(canonical.status).toBe(307);
  expect(canonical.headers.get("location")).toBe(
    "https://review.example.test/nl",
  );
  expect(canonical.headers.get("vary")).toBeNull();

  for (const path of [
    "/nl",
    "/nl/collection",
    "/nl/collection/seating",
    "/nl/winkelinformatie",
    "/catalogue-images/nl/SET-COVERS/1/640",
    "/robots.txt",
  ]) {
    for (const method of ["GET", "HEAD"]) {
      const response = proxy(
        new NextRequest(`https://review.example.test${path}`, { method }),
      );
      expect(response.status).toBe(200);
      expect(response.headers.get("www-authenticate")).toBeNull();
      expect(response.headers.get("cache-control")).toBe("private, no-store");
      expect(response.headers.get("x-robots-tag")).toBe("noindex, nofollow");
      expect(response.headers.get("content-security-policy")).toContain(
        "form-action 'self'",
      );
    }
  }

  const authorization = `Basic ${Buffer.from("test-admin:" + reviewEnvironment.STAGING_ADMIN_TOKEN).toString("base64")}`;
  for (const path of [
    "/en/collection",
    "/nl/gallery",
    "/nl/room-planner",
    "/nl/products/demo-seating",
    "/nl/catalogue.xml",
    "/nl/admin",
    "/nl/%61dmin",
    "/nl/checkout",
    "/api/catalogue/nl",
    "/api/published/nl",
    "/catalogue-images/de/SET-COVERS/1/640",
  ]) {
    const blocked = proxy(request(path, { authorization }));
    expect(blocked.status).toBe(404);
    expect(blocked.headers.get("www-authenticate")).toBeNull();
    expect(blocked.headers.get("x-robots-tag")).toBe("noindex, nofollow");
  }
  expect(
    proxy(
      new NextRequest("https://review.example.test/nl/collection", {
        method: "POST",
        headers: { authorization },
      }),
    ).status,
  ).toBe(404);
});

it("rejects incomplete and out-of-scope store review configuration", () => {
  expect(configuration(reviewEnvironment).storeReview).toEqual(reviewContent);
  expect(
    configuration({ ...reviewEnvironment, STAGING_STORE_REVIEW_CONTENT: "" })
      .storeReview,
  ).toBeNull();
  expect(() =>
    configuration({ ...reviewEnvironment, APP_ENV: "test" }),
  ).toThrow();
  expect(() =>
    configuration({ ...reviewEnvironment, STAGING_PUBLIC_STOREFRONT: "false" }),
  ).toThrow();
  for (const content of [
    "{",
    "null",
    " ".repeat(48001),
    JSON.stringify({ ...reviewContent, sellerName: "" }),
    JSON.stringify({ ...reviewContent, address: " " }),
    JSON.stringify({ ...reviewContent, kvk: "missing" }),
    JSON.stringify({ ...reviewContent, vatId: "NL0" }),
    JSON.stringify({ ...reviewContent, phone: "+31 123456789" }),
    JSON.stringify({ ...reviewContent, email: "name@example.test?body=x" }),
    JSON.stringify({
      ...reviewContent,
      policies: { ...reviewContent.policies, payment: undefined },
    }),
    JSON.stringify({
      ...reviewContent,
      policies: { ...reviewContent.policies, delivery: "" },
    }),
    JSON.stringify({ ...reviewContent, privateToken: "private" }),
  ])
    expect(() =>
      configuration({
        ...reviewEnvironment,
        STAGING_STORE_REVIEW_CONTENT: content,
      }),
    ).toThrow("Invalid store review content");
});
