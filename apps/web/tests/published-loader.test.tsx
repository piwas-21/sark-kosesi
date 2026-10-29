import { it, expect, vi } from "vitest";
vi.mock("server-only", () => ({}));
import {
  publishedPage,
  publishedProduct,
  publishedCollection,
} from "../src/lib/load-published";
import { approved, page } from "./published-fixture";
function environment() {
  vi.stubEnv("APP_ENV", "test");
  vi.stubEnv("PREVIEW_TOKEN", "a".repeat(32));
  vi.stubEnv("API_URL", "http://127.0.0.1:4000");
  vi.stubEnv("CANONICAL_ORIGIN", "http://127.0.0.1:3000");
}
function serve(value: unknown, status = 200) {
  environment();
  const fetch = vi
    .fn()
    .mockImplementation(() =>
      Promise.resolve(Response.json(value, { status })),
    );
  vi.stubGlobal("fetch", fetch);
  return fetch;
}
it("uses server credentials, no-store and exact locale/SKU identities", async () => {
  const fetch = serve(page());
  expect((await publishedPage("nl")).items[0]).toEqual(approved);
  const request = fetch.mock.calls[0]?.[0];
  expect(request).toBeInstanceOf(Request);
  if (!(request instanceof Request)) throw new Error("Missing request");
  expect(request.headers.get("authorization")).toBe(`Bearer ${"a".repeat(32)}`);
  expect(request.cache).toBe("no-store");
  serve(approved);
  expect(await publishedProduct("nl", "SET-COVERS")).toEqual(approved);
  serve({
    ...approved,
    sku: "WRONG-SKU",
    images: [
      {
        ...approved.images[0],
        url: "/api/published/nl/images/WRONG-SKU/1/320",
      },
    ],
  });
  await expect(publishedProduct("nl", "SET-COVERS")).rejects.toThrow(
    "identity mismatch",
  );
  serve(approved);
  await expect(publishedProduct("ar", "SET-COVERS")).rejects.toThrow(
    "identity mismatch",
  );
  serve(page());
  await expect(publishedPage("ar")).rejects.toThrow("mismatch");
  const unused = serve({});
  expect(await publishedProduct("nl", "bad/path")).toBeNull();
  expect(unused).not.toHaveBeenCalled();
  serve({}, 404);
  expect(await publishedProduct("nl", "MISSING")).toBeNull();
  for (const load of [
    () => publishedPage("nl"),
    () => publishedProduct("nl", "SET-COVERS"),
  ]) {
    serve({}, 503);
    await expect(load()).rejects.toThrow("unavailable");
    serve({});
    await expect(load()).rejects.toThrow("Invalid");
  }
});
it("walks every page and rejects repeated or reverse cursors instead of returning partial feeds", async () => {
  environment();
  const fetch = vi
    .fn()
    .mockImplementationOnce(() =>
      Promise.resolve(Response.json(page([approved], "SET-COVERS"))),
    )
    .mockImplementationOnce(() => Promise.resolve(Response.json(page([]))));
  vi.stubGlobal("fetch", fetch);
  expect(await publishedCollection("nl")).toEqual([approved]);
  expect(fetch).toHaveBeenCalledTimes(2);
  serve(page([approved], "SET-COVERS"));
  await expect(publishedPage("nl", "SET-COVERS")).rejects.toThrow(
    "cursor mismatch",
  );
  serve(page([], "AAA"));
  await expect(publishedPage("nl", "BBB")).rejects.toThrow("cursor mismatch");
  let number = 0;
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockImplementation(() =>
        Promise.resolve(
          Response.json(page([], `PAGE-${String(number++).padStart(2, "0")}`)),
        ),
      ),
  );
  await expect(publishedCollection("nl")).rejects.toThrow(
    "exceeds preview limit",
  );
});
