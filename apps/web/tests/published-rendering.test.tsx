import { it, expect, vi } from "../../../tests/application/unit-runtime.mjs";
import { renderToStaticMarkup } from "react-dom/server";
import { publishedPage, publishedCollection } from "../src/lib/load-published";
import Collection from "../src/app/[locale]/collection/page";
import Family, {
  generateMetadata,
} from "../src/app/[locale]/collection/[slug]/page";
import { GET as feed } from "../src/app/[locale]/catalogue.xml/route";
import { GET as image } from "../src/app/catalogue-images/[locale]/[sku]/[revision]/[edge]/route";
import StoreInformation from "../src/app/[locale]/winkelinformatie/page";
import { reviewContent, reviewEnvironment } from "./store-review-fixture";
import { approved, page, dimensions } from "./published-fixture";
import type { PublishedProduct } from "../src/lib/published-catalogue";
const filled: PublishedProduct = {
  ...approved,
  sku: "SET-FILLED",
  fill: "with-foam",
  minor_units: 54321,
  price_decimal: "543.21",
  availability: "out_of_stock",
  components: [
    ...approved.components,
    {
      code: "SEAT-FOAM",
      quantity: 2,
      dimensions,
      material: { kind: "foam", density_g_per_m3: 35000 },
    },
  ],
  images: [
    { ...approved.images[0], url: "/api/published/nl/images/SET-FILLED/1/320" },
  ],
};
function environment() {
  vi.stubEnv("APP_ENV", "test");
  vi.stubEnv("PREVIEW_TOKEN", "a".repeat(32));
  vi.stubEnv("API_URL", "http://127.0.0.1:4000");
  vi.stubEnv("CANONICAL_ORIGIN", "http://127.0.0.1:3000");
}
const family = (variant?: string, slug = "isolated-room") => ({
  params: Promise.resolve({ locale: "nl", slug }),
  searchParams: Promise.resolve(variant === undefined ? {} : { variant }),
});
it("renders exact selected facts as escaped HTML and genuine variant links", async () => {
  environment();
  vi.mocked(publishedCollection).mockResolvedValue([approved, filled]);
  for (const variant of [undefined, "SET-COVERS", "SET-FILLED"]) {
    const html = renderToStaticMarkup(await Family(family(variant)));
    const doc = new DOMParser().parseFromString(html, "text/html");
    const selected = variant === "SET-FILLED" ? filled : approved;
    expect(doc.querySelector("[data-price]")?.getAttribute("data-price")).toBe(
      selected.price_decimal,
    );
    expect(
      doc
        .querySelector("[data-availability]")
        ?.getAttribute("data-availability"),
    ).toBe(selected.availability);
    expect(
      doc.querySelector('[aria-current="true"]')?.getAttribute("href"),
    ).toContain(`variant=${selected.sku}`);
    expect(doc.body.textContent).toContain(selected.copy.care);
    expect(doc.body.textContent).toContain(selected.copy.safety);
    expect(doc.body.textContent).toContain(selected.copy.assembly);
    expect(doc.body.textContent).toContain("3000 g");
    expect(doc.body.textContent).toContain("1000 × 500 × 100 mm");
    expect(doc.querySelector("script")).toBeNull();
    expect(doc.querySelector("img")?.getAttribute("src")).toBe(
      `/catalogue-images/nl/${selected.sku}/1/320`,
    );
  }
  expect(renderToStaticMarkup(await Family(family("SET-FILLED")))).toContain(
    "35000 g/m³",
  );
  await expect(Family(family("MISSING"))).rejects.toThrow("NOT_FOUND");
  await expect(Family(family("bad/path"))).rejects.toThrow("NOT_FOUND");
  await expect(Family(family("SET-COVERS", "wrong-slug"))).rejects.toThrow(
    "REDIRECT:/nl/collection/isolated-room?variant=SET-COVERS",
  );
  await expect(
    Family({
      ...family(),
      params: Promise.resolve({ locale: "bad", slug: "isolated-room" }),
    }),
  ).rejects.toThrow("NOT_FOUND");
  vi.mocked(publishedCollection).mockResolvedValue([]);
  await expect(Family(family())).rejects.toThrow("NOT_FOUND");
  expect(await generateMetadata(family())).toMatchObject({
    alternates: {
      canonical: "http://127.0.0.1:3000/nl/collection/isolated-room",
    },
  });
  expect(
    await generateMetadata({
      ...family(),
      params: Promise.resolve({ locale: "bad", slug: "bad" }),
    }),
  ).toEqual({});
});
it("renders empty and paginated collection responses without pretending missing copy exists", async () => {
  environment();
  vi.mocked(publishedPage)
    .mockClear()
    .mockResolvedValue({
      ...page([approved, filled], "SET-FILLED"),
      production_enabled: false,
    });
  const html = renderToStaticMarkup(
    await Collection({
      params: Promise.resolve({ locale: "nl" }),
      searchParams: Promise.resolve({}),
    }),
  );
  expect(html).toContain("variant=SET-COVERS");
  expect(html).toContain("after=SET-FILLED");
  expect(vi.mocked(publishedPage).mock.calls).toEqual([["nl", undefined]]);
  expect(html).toContain("Preview catalogue feed");
  expect(html).not.toContain("<script>");
  vi.mocked(publishedPage).mockResolvedValue({
    ...page([]),
    production_enabled: false,
  });
  expect(
    renderToStaticMarkup(
      await Collection({
        params: Promise.resolve({ locale: "nl" }),
        searchParams: Promise.resolve({}),
      }),
    ),
  ).toContain("No product details");
  for (const [locale, after] of [
    ["bad", "AAA"],
    ["nl", "bad/path"],
  ])
    await expect(
      Collection({
        params: Promise.resolve({ locale: locale ?? "" }),
        searchParams: Promise.resolve({ after: after ?? "" }),
      }),
    ).rejects.toThrow("NOT_FOUND");
});
it("serves a complete private XML feed or a redacted failure", async () => {
  environment();
  vi.mocked(publishedCollection).mockResolvedValue([approved]);
  let response = await feed(new Request("http://local/nl/catalogue.xml"), {
    params: Promise.resolve({ locale: "nl" }),
  });
  expect(response.status).toBe(200);
  expect(response.headers.get("cache-control")).toContain("no-store");
  const xml = await response.text();
  expect(xml).toContain('<price currency="EUR">123.45</price>');
  expect(xml).toContain("variant=SET-COVERS");
  response = await feed(new Request("http://local/bad/catalogue.xml"), {
    params: Promise.resolve({ locale: "bad" }),
  });
  expect(response.status).toBe(404);
  vi.mocked(publishedCollection).mockRejectedValue(
    new Error("private_database_password"),
  );
  response = await feed(new Request("http://local/nl/catalogue.xml"), {
    params: Promise.resolve({ locale: "nl" }),
  });
  expect(response.status).toBe(503);
  expect(await response.text()).not.toContain("private_database_password");
});
it("proxies only exact revision-bound sanitized raster paths with bounded private responses", async () => {
  environment();
  const params = {
    locale: "nl",
    sku: "SET-COVERS",
    revision: "1",
    edge: "320",
  };
  const call = (p = params) =>
    image(new Request("http://local/image"), { params: Promise.resolve(p) });
  for (const patch of [
    { locale: "kmr" },
    { sku: "DEMO-COVER" },
    { revision: "0" },
    { revision: "2147483648" },
    { edge: "321" },
  ])
    expect((await call({ ...params, ...patch })).status).toBe(400);
  const fetch = vi.fn().mockImplementation(() =>
    Promise.resolve(
      new Response(new Uint8Array([1, 2, 3]), {
        headers: { "content-type": "image/png" },
      }),
    ),
  );
  vi.stubGlobal("fetch", fetch);
  let response = await call();
  expect(response.status).toBe(200);
  expect(response.headers.get("content-type")).toBe("image/png");
  expect(response.headers.get("cache-control")).toContain("no-store");
  expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  expect(fetch.mock.calls[0]?.[0]).toBe(
    "http://127.0.0.1:4000/api/published/nl/images/SET-COVERS/1/320",
  );
  for (const [status, type, body] of [
    [404, "text/plain", "private"],
    [503, "text/plain", "private"],
    [200, "text/html", "<script>"],
    [200, "image/jpeg", ""],
  ]) {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation(() =>
        Promise.resolve(
          new Response(String(body), {
            status: Number(status),
            headers: { "content-type": String(type) },
          }),
        ),
      ),
    );
    response = await call();
    expect(response.status).toBe(status === 404 ? 404 : 503);
    expect(await response.text()).toBe("Image unavailable");
  }
  vi.stubGlobal(
    "fetch",
    vi.fn().mockRejectedValue(new Error("private_dependency")),
  );
  expect((await call()).status).toBe(503);
});
it("explains an unavailable configuration without substituting another SKU", async () => {
  environment();
  const { default: Unavailable } =
    await import("../src/app/[locale]/collection/not-found");
  expect(renderToStaticMarkup(<Unavailable />)).toContain(
    "Configuration unavailable",
  );
});

it("keeps loading status scoped to specimen routes", async () => {
  const { default: ProductsLoading } =
    await import("../src/app/[locale]/products/loading");
  const { default: GalleryLoading } =
    await import("../src/app/[locale]/gallery/loading");
  expect(renderToStaticMarkup(<ProductsLoading />)).toContain(
    "Loading preview",
  );
  expect(renderToStaticMarkup(<GalleryLoading />)).toContain("Loading preview");
});

it("serves the current Dutch catalogue, variant and review policies without demo or checkout links", async () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);
  const dutch = { params: Promise.resolve({ locale: "nl" }) };
  const collection = { ...dutch, searchParams: Promise.resolve({}) };
  vi.mocked(publishedPage).mockResolvedValue(page([approved]));
  vi.mocked(publishedCollection).mockResolvedValue([approved, filled]);

  const collectionHtml = renderToStaticMarkup(await Collection(collection));
  expect(collectionHtml).toContain("De collectie");
  expect(collectionHtml).toContain(approved.copy.title);
  expect(collectionHtml).toContain("123,45");
  expect(collectionHtml).toContain("incl. btw");
  expect(collectionHtml).toContain("Alleen hoezen, zonder schuim");
  expect(collectionHtml).not.toContain("Preview catalogue feed");
  expect(collectionHtml).not.toContain("/checkout");
  const productHtml = renderToStaticMarkup(await Family(family("SET-FILLED")));
  expect(productHtml).toContain("Met schuimvulling");
  expect(productHtml).toContain("543,21");
  expect(productHtml).toContain("Online bestellen is nog niet beschikbaar.");
  expect(productHtml).toContain("/nl/winkelinformatie#retourneren");
  expect(productHtml).not.toContain("Private preview");

  const information = renderToStaticMarkup(await StoreInformation(dutch));
  const doc = new DOMParser().parseFromString(information, "text/html");
  expect(
    doc.querySelector('a[href="mailto:contact@example.test"]'),
  ).not.toBeNull();
  expect(doc.querySelector('a[href="tel:+31123456789"]')).not.toBeNull();
  expect(doc.body.textContent).toContain("01234567");
  expect(doc.body.textContent).toContain("NL012345678B01");
  for (const [anchor, text] of [
    ["bezorging", reviewContent.policies.delivery],
    ["retourneren", reviewContent.policies.returns],
    ["privacy", reviewContent.policies.privacy],
    ["betalen", reviewContent.policies.payment],
    ["voorwaarden", reviewContent.policies.terms],
  ])
    expect(doc.getElementById(anchor ?? "")?.textContent).toContain(text);
  expect(doc.querySelector("script")).toBeNull();

  const { default: Unavailable } =
    await import("../src/app/[locale]/collection/not-found");
  expect(renderToStaticMarkup(<Unavailable />)).toContain(
    "Product niet beschikbaar",
  );

  vi.mocked(publishedPage).mockResolvedValue(page([]));
  await expect(Collection(collection)).rejects.toThrow("NOT_FOUND");
  await expect(
    StoreInformation({ params: Promise.resolve({ locale: "en" }) }),
  ).rejects.toThrow("NOT_FOUND");
  vi.stubEnv("STAGING_STORE_REVIEW_CONTENT", "");
  await expect(StoreInformation(dutch)).rejects.toThrow("NOT_FOUND");
});

it("continues across empty historical Dutch catalogue windows within the existing bound", async () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);
  const args = {
    params: Promise.resolve({ locale: "nl" }),
    searchParams: Promise.resolve({}),
  };
  vi.mocked(publishedPage)
    .mockClear()
    .mockResolvedValueOnce(page([], "ARCHIVED-A"))
    .mockResolvedValueOnce(page([], "ARCHIVED-B"))
    .mockResolvedValueOnce(page([approved]));
  expect(renderToStaticMarkup(await Collection(args))).toContain(
    approved.copy.title,
  );
  expect(vi.mocked(publishedPage).mock.calls).toEqual([
    ["nl", undefined],
    ["nl", "ARCHIVED-A"],
    ["nl", "ARCHIVED-B"],
  ]);

  vi.mocked(publishedPage)
    .mockResolvedValueOnce(page([], "ARCHIVED-A"))
    .mockResolvedValueOnce(page([]));
  await expect(Collection(args)).rejects.toThrow("NOT_FOUND");

  vi.mocked(publishedPage)
    .mockClear()
    .mockImplementation(async (_locale, after) =>
      page([], after === undefined ? "ARCHIVED" : after + "A"),
    );
  await expect(Collection(args)).rejects.toThrow(
    "Published collection exceeds preview limit",
  );
  expect(vi.mocked(publishedPage)).toHaveBeenCalledTimes(40);
  await expect(
    Collection({ ...args, params: Promise.resolve({ locale: "en" }) }),
  ).rejects.toThrow("NOT_FOUND");
});

it("omits a Dutch review continuation when only ineligible history follows", async () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);
  const args = {
    params: Promise.resolve({ locale: "nl" }),
    searchParams: Promise.resolve({}),
  };
  vi.mocked(publishedPage)
    .mockClear()
    .mockResolvedValueOnce(page([approved], "SET-CURSOR"))
    .mockResolvedValueOnce(page([], "Z-ARCHIVED"))
    .mockResolvedValueOnce(page([]));

  const html = renderToStaticMarkup(await Collection(args));
  expect(html).toContain(approved.copy.title);
  expect(html).not.toContain("Meer uit de collectie");
  expect(vi.mocked(publishedPage).mock.calls).toEqual([
    ["nl", undefined],
    ["nl", "SET-CURSOR"],
    ["nl", "Z-ARCHIVED"],
  ]);
});

it("preserves the raw Dutch review cursor when later eligible products exist", async () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);
  const args = {
    params: Promise.resolve({ locale: "nl" }),
    searchParams: Promise.resolve({}),
  };
  vi.mocked(publishedPage)
    .mockClear()
    .mockResolvedValueOnce(page([approved], "SET-CURSOR"))
    .mockResolvedValueOnce(page([], "SET-DORMANT"))
    .mockResolvedValueOnce(page([filled], "Z-ARCHIVED"));

  const html = renderToStaticMarkup(await Collection(args));
  expect(html).toContain(approved.copy.title);
  expect(html).not.toContain("variant=SET-FILLED");
  expect(html).toContain("/nl/collection?after=SET-CURSOR");
  expect(html).toContain("Meer uit de collectie");
  expect(vi.mocked(publishedPage).mock.calls).toEqual([
    ["nl", undefined],
    ["nl", "SET-CURSOR"],
    ["nl", "SET-DORMANT"],
  ]);
});

it("fails an unproved Dutch review continuation within the shared history budget", async () => {
  for (const [name, value] of Object.entries(reviewEnvironment))
    vi.stubEnv(name, value);
  const args = {
    params: Promise.resolve({ locale: "nl" }),
    searchParams: Promise.resolve({}),
  };
  vi.mocked(publishedPage)
    .mockClear()
    .mockResolvedValueOnce(page([], "ARCHIVED"))
    .mockResolvedValueOnce(page([approved], "SET-CURSOR"))
    .mockImplementation(async (_locale, after) =>
      page([], after === undefined ? "Z" : after + "Z"),
    );

  await expect(Collection(args)).rejects.toThrow(
    "Published collection exceeds preview limit",
  );
  expect(vi.mocked(publishedPage)).toHaveBeenCalledTimes(40);
});
