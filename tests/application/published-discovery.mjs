import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { spawnSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
const base = process.env.WEB_BASE_URL;
if (!base || !process.env.PREVIEW_TOKEN || process.env.APP_ENV !== "test")
  throw new Error("Owned browser fixture required");
const authorization = `Basic ${Buffer.from(`preview:${process.env.PREVIEW_TOKEN}`).toString("base64")}`;
const observations = [];
async function get(path, status = 200, extra = {}) {
  const response = await fetch(base + path, {
    headers: {
      Authorization: authorization,
      "User-Agent": "HTML-limited-crawler",
      ...extra,
    },
    redirect: "manual",
  });
  assert.equal(response.status, status, path);
  assert.match(response.headers.get("cache-control") ?? "", /no-store/);
  assert.match(response.headers.get("x-robots-tag") ?? "", /noindex/);
  return response;
}
function inspect(html, sku, price, availability) {
  const doc = new JSDOM(html).window.document;
  assert.match(doc.querySelector("h1")?.textContent ?? "", new RegExp(sku));
  assert.equal(
    doc.querySelector("[data-price]")?.getAttribute("data-price"),
    price,
  );
  const visiblePrice = {
    98.76: /€\s*98,76/,
    111.11: /€\s*111,11/,
    123.45: /€\s*123,45/,
  };
  assert.match(
    doc.querySelector("[data-price]")?.textContent ?? "",
    visiblePrice[price],
  );
  assert.equal(
    doc.querySelector("[data-availability]")?.getAttribute("data-availability"),
    availability,
  );
  assert.match(
    doc.querySelector('[aria-current="true"]')?.getAttribute("href") ?? "",
    new RegExp(`variant=${sku}$`),
  );
  assert.match(doc.body.textContent ?? "", /SEAT-COVER/);
  assert.match(doc.body.textContent ?? "", /3000 g/);
  assert.match(doc.body.textContent ?? "", /1000 × 500 × 100 mm/);
  assert.equal(
    doc.querySelectorAll('link[rel="alternate"][hreflang]').length,
    0,
  );
  assert.equal(
    doc.querySelectorAll('script[type="application/ld+json"]').length,
    0,
    "Non-selling fixtures must not advertise merchant offers",
  );
  assert.equal(
    doc.querySelector('link[rel="canonical"]')?.getAttribute("href"),
    base + "/nl/collection/isolated-room",
  );
  for (const privateValue of [
    process.env.PREVIEW_TOKEN,
    "token_hash",
    "qualification_ref",
    "synthetic workflow exercise",
    "catalogue_media_assets",
  ])
    assert.ok(!html.includes(privateValue));
  return doc;
}
function inspectFeed(text, sku, price, availability) {
  const doc = new JSDOM(text, { contentType: "text/xml" }).window.document;
  assert.equal(doc.documentElement.getAttribute("preview-only"), "true");
  assert.equal(doc.documentElement.getAttribute("production-enabled"), "false");
  const item = [...doc.querySelectorAll("product")].find(
    (p) => p.querySelector("sku")?.textContent === sku,
  );
  assert.ok(item, `Missing ${sku}`);
  assert.equal(item.querySelector("price")?.textContent, price);
  assert.equal(item.querySelector("price")?.getAttribute("currency"), "EUR");
  assert.equal(item.querySelector("availability")?.textContent, availability);
  assert.match(
    item.querySelector("link")?.textContent ?? "",
    new RegExp(`variant=${sku}$`),
  );
  return item;
}
async function check(price = "98.76", availability = "in_stock") {
  const html = await (
    await get("/nl/collection/isolated-room?variant=SET-COVERS")
  ).text();
  const doc = inspect(html, "SET-COVERS", price, availability);
  const text = await (await get("/nl/catalogue.xml")).text();
  const item = inspectFeed(text, "SET-COVERS", price, availability);
  const image = doc.querySelector("img")?.getAttribute("src");
  assert.equal(base + image, item.querySelector("image")?.textContent);
  const response = await get(image);
  assert.equal(response.headers.get("content-type"), "image/jpeg");
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
  const bytes = new Uint8Array(await response.arrayBuffer());
  assert.ok(bytes.byteLength > 0 && bytes.byteLength <= 250 * 1024);
  assert.equal(bytes[0], 255);
  assert.equal(bytes[1], 216);
  observations.push({ sku: "SET-COVERS", price, availability, image });
  return { html, feed: text, image };
}
function effect(action) {
  const result = spawnSync(
    "cargo",
    [
      "run",
      "--locked",
      "--quiet",
      "-p",
      "commerce-infrastructure",
      "--example",
      "browser-fixture-effect",
      "--",
      action,
    ],
    { env: process.env, encoding: "utf8", timeout: 120000 },
  );
  if (result.status !== 0)
    throw new Error(`Owned fixture effect failed: ${action}; details withheld`);
}
await mkdir(".local/evidence", { recursive: true });
const initial = await check();
await writeFile(".local/evidence/published-product.html", initial.html);
await writeFile(".local/evidence/published-catalogue.xml", initial.feed);
const filled = await (
  await get("/nl/collection/isolated-room?variant=SET-FILLED")
).text();
inspect(filled, "SET-FILLED", "123.45", "out_of_stock");
assert.match(filled, /35000/);
for (const locale of ["en", "ku"]) {
  const xml = await (await get(`/${locale}/catalogue.xml`)).text();
  assert.equal(
    new JSDOM(xml, {
      contentType: "text/xml",
    }).window.document.querySelectorAll("product").length,
    0,
  );
  assert.match(
    await (await get(`/${locale}/collection`)).text(),
    /No product details/,
  );
}
const arabic = await (
  await get("/ar/collection/isolated-ar-room?variant=SET-COVERS")
).text();
const ardoc = new JSDOM(arabic).window.document;
assert.equal(ardoc.documentElement.dir, "rtl");
assert.equal(ardoc.documentElement.lang, "ar");
assert.match(ardoc.querySelector("h1")?.textContent ?? "", /عينة اختبار/);
inspectFeed(
  await (await get("/ar/catalogue.xml")).text(),
  "SET-COVERS",
  "98.76",
  "in_stock",
);
await get("/nl/collection/isolated-room?variant=MISSING", 404);
await get("/catalogue-images/nl/PHOTO/1/320", 404);
await get("/catalogue-images/nl/SET-COVERS/1/0", 400);
inspect(
  await (
    await get("/nl/collection/isolated-room?variant=SET-COVERS", 200, {
      "x-forwarded-host": "attacker.invalid",
    })
  ).text(),
  "SET-COVERS",
  "98.76",
  "in_stock",
);
for (const corrupt of [
  initial.html.replace('data-price="98.76"', 'data-price="999.00"'),
  initial.html.replaceAll("98,76", "999,00"),
  initial.html.replace(
    'data-availability="in_stock"',
    'data-availability="out_of_stock"',
  ),
])
  assert.throws(() => inspect(corrupt, "SET-COVERS", "98.76", "in_stock"));
assert.throws(() =>
  inspectFeed(
    initial.feed.replace(">98.76<", ">999.00<"),
    "SET-COVERS",
    "98.76",
    "in_stock",
  ),
);
effect("quarantine");
await check("98.76", "out_of_stock");
effect("release");
await check();
effect("price");
await get("/nl/collection/isolated-room?variant=SET-COVERS", 404);
await get(initial.image, 404);
assert.doesNotMatch(
  await (await get("/nl/catalogue.xml")).text(),
  /<sku>SET-COVERS<\/sku>/,
);
effect("republish");
const republished = await check("111.11");
assert.notEqual(republished.image, initial.image);
await get(initial.image, 404);
effect("withdraw");
await get("/nl/collection/isolated-room?variant=SET-COVERS", 404);
await get(republished.image, 404);
assert.doesNotMatch(
  await (await get("/nl/catalogue.xml")).text(),
  /<sku>SET-COVERS<\/sku>/,
);
effect("restore");
await check();
await get(initial.image, 404);
await writeFile(
  ".local/evidence/published-discovery.json",
  JSON.stringify(
    {
      privatePreview: true,
      productionEnabled: false,
      observations,
      negativeControls: 4,
      readThroughEffects: [
        "quarantine",
        "release",
        "price draft",
        "explicit republication",
        "withdrawal",
        "explicit restore",
      ],
      actualMerchantAcceptance: false,
    },
    null,
    2,
  ),
);
console.log(
  "Approved preview HTML/XML/images agree; real stock/draft/publication changes invalidate subsequent requests. Fixtures do not establish merchant or launch acceptance.",
);
