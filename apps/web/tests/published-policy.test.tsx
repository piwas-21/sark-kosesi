import { it, expect } from "vitest";
import {
  decodePublishedProduct,
  decodePublishedCatalogue,
} from "../src/lib/published-catalogue";
import {
  decimal,
  merchantCode,
  measurements,
} from "../src/lib/published-validation";
import {
  catalogueFeed,
  displayPrice,
  productPath,
  imagePath,
  xml,
} from "../src/lib/published-presentation";
import { approved, page, dimensions } from "./published-fixture";
it("validates a closed approved projection independently of private authoring data", () => {
  expect(decodePublishedProduct(approved)).toEqual(approved);
  expect(decodePublishedCatalogue(page())).toEqual(page());
  expect(decodePublishedCatalogue(page([]))).toEqual(page([]));
  const foam = {
    ...approved,
    fill: "with-foam",
    components: [
      ...approved.components,
      {
        code: "SEAT-FOAM",
        quantity: 2,
        dimensions,
        material: { kind: "foam", density_g_per_m3: 35000 },
      },
    ],
  };
  expect(decodePublishedProduct(foam).fill).toBe("with-foam");
  for (const value of [
    null,
    [],
    {},
    true,
    { ...approved, source_ref: "private" },
    { ...approved, sku: "DEMO-SAMPLE" },
    { ...approved, locale: "kmr" },
    { ...approved, family_code: "bad" },
    { ...approved, family_slug: "../secret" },
    { ...approved, size_code: "" },
    { ...approved, textile_code: 1 },
    { ...approved, fill: "prepacked" },
    { ...approved, minor_units: -1 },
    { ...approved, minor_units: Number.MAX_SAFE_INTEGER + 1 },
    { ...approved, price_decimal: "123.46" },
    { ...approved, currency: "USD" },
    { ...approved, copy: { ...approved.copy, title: "" } },
    { ...approved, copy: { ...approved.copy, materials: 1 } },
    { ...approved, copy: { ...approved.copy, safety: "\0" } },
    { ...approved, footprint: { ...dimensions, tolerance_mm: 100 } },
    { ...approved, availability: "maybe" },
    { ...approved, publication_revision: 0 },
  ])
    expect(() => decodePublishedProduct(value)).toThrow();
});
it("rejects malformed component, parcel and rendition payloads", () => {
  for (const component of [
    null,
    {},
    { ...approved.components[0], quantity: 0 },
    { ...approved.components[0], code: "DEMO-COVER" },
    { ...approved.components[0], material: { kind: "cover", fibres: [] } },
    {
      ...approved.components[0],
      material: { kind: "cover", fibres: [{ name: "", basis_points: 10000 }] },
    },
    {
      ...approved.components[0],
      material: { kind: "foam", density_g_per_m3: 0 },
    },
    { ...approved.components[0], material: { kind: "unknown" } },
  ])
    expect(() =>
      decodePublishedProduct({ ...approved, components: [component] }),
    ).toThrow();
  for (const field of ["components", "parcels", "images"])
    for (const value of [null, [], Array(65).fill({})])
      expect(() =>
        decodePublishedProduct({ ...approved, [field]: value }),
      ).toThrow();
  for (const value of [
    { ...approved.parcels[0], gross_weight_g: 0 },
    { dimensions: {}, gross_weight_g: 1 },
    null,
  ])
    expect(() =>
      decodePublishedProduct({ ...approved, parcels: [value] }),
    ).toThrow();
  for (const value of [
    { ...approved.images[0], url: "https://attacker.invalid/original" },
    { ...approved.images[0], edge: 321 },
    { ...approved.images[0], width: 0 },
    { ...approved.images[0], height: 961 },
    null,
  ])
    expect(() =>
      decodePublishedProduct({ ...approved, images: [value] }),
    ).toThrow();
  for (const v of [
    null,
    [],
    {},
    { ...page(), locale: "kmr" },
    { ...page(), items: {} },
    { ...page(), items: Array(26).fill(approved) },
    { ...page(), production_enabled: true },
    { ...page(), next_after: "bad" },
    { ...page(), items: [approved, approved] },
    { ...page(), next_after: "AAA" },
    { ...page(), locale: "ar" },
    { ...page(), actor: "private" },
  ])
    expect(() => decodePublishedCatalogue(v)).toThrow();
  expect(decodePublishedCatalogue(page([], "SET-COVERS")).next_after).toBe(
    "SET-COVERS",
  );
  expect(merchantCode("A")).toBe(true);
  expect(merchantCode("DEMO")).toBe(false);
  expect(measurements(null)).toBe(false);
});
it("formats maximum exact minor units and XML without rounding or executable copy", () => {
  expect(decimal(9007199254740991)).toBe("90071992547409.91");
  expect(decimal(0)).toBe("0.00");
  expect(displayPrice(9007199254740991, "en")).toContain(
    "90,071,992,547,409.91",
  );
  expect(displayPrice(1, "nl")).toContain("0,01");
  expect(displayPrice(12345, "ar")).toBeTruthy();
  expect(displayPrice(12345, "ku")).toBeTruthy();
  expect(productPath(approved)).toBe(
    "/nl/collection/isolated-room?variant=SET-COVERS",
  );
  expect(imagePath(approved.images[0]?.url ?? "")).toBe(
    "/catalogue-images/nl/SET-COVERS/1/320",
  );
  expect(xml(`<a key="a'b">&`)).toBe("&lt;a key=&quot;a&apos;b&quot;&gt;&amp;");
  for (const value of ["\u0001", "\uD800", "\uFFFF"])
    expect(() => xml(value)).toThrow();
  expect(xml("العربية\n😀\t\r")).toBe("العربية\n😀\t\r");
  const feed = catalogueFeed([approved], "http://127.0.0.1:3000", "nl");
  const doc = new DOMParser().parseFromString(feed, "application/xml");
  expect(doc.querySelector("parsererror")).toBeNull();
  expect(doc.querySelector("sku")?.textContent).toBe("SET-COVERS");
  expect(doc.querySelector("price")?.textContent).toBe("123.45");
  expect(doc.querySelector("availability")?.textContent).toBe("in_stock");
  expect(doc.querySelector("description")?.textContent).toContain("<script>");
  expect(doc.querySelector("script")).toBeNull();
  expect(doc.documentElement.getAttribute("preview-only")).toBe("true");
});
it("bounds streamed image reads and cancels oversized upstream bodies", async () => {
  const { readImage } = await import("../src/lib/read-image");
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new Uint8Array([1, 2]));
      controller.enqueue(new Uint8Array([3]));
      controller.close();
    },
  });
  expect([...new Uint8Array(await readImage(new Response(stream)))]).toEqual([
    1, 2, 3,
  ]);
  let cancelled = false;
  const oversized = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new Uint8Array(250 * 1024 + 1));
    },
    cancel() {
      cancelled = true;
    },
  });
  await expect(readImage(new Response(oversized))).rejects.toThrow(
    "size exceeded",
  );
  expect(cancelled).toBe(true);
  await expect(readImage(new Response(null))).rejects.toThrow(
    "Missing image body",
  );
  await expect(readImage(new Response(""))).rejects.toThrow("Empty image body");
  expect(() => imagePath("/original/private")).toThrow("Invalid image URL");
});

it("rejects unsafe or negative input at the public money formatter", () => {
  for (const value of [-1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1])
    expect(() => displayPrice(value, "nl")).toThrow("Invalid public money");
});

it("preserves the rendered default SKU and existing parameters across locale family paths", async () => {
  const { switchLocale } = await import("../src/lib/locale");
  expect(
    switchLocale("/nl/collection/isolated-room", "", "ar", "SET-COVERS"),
  ).toBe("/ar/collection/isolated-room?variant=SET-COVERS");
  expect(
    switchLocale(
      "/nl/collection/isolated-room",
      "?token=required",
      "ar",
      "SET-COVERS",
    ),
  ).toBe("/ar/collection/isolated-room?token=required&variant=SET-COVERS");
  expect(
    switchLocale(
      "/nl/collection/isolated-room",
      "?variant=SET-FILLED",
      "ar",
      "SET-COVERS",
    ),
  ).toBe("/ar/collection/isolated-room?variant=SET-FILLED");
  expect(
    switchLocale("/nl/collection/isolated-room", "", "ar", "bad/path"),
  ).toBe("/ar/collection/isolated-room");
  expect(
    switchLocale("/nl/gallery", "?token=required", "ar", "SET-COVERS"),
  ).toBe("/ar/gallery?token=required");
});
