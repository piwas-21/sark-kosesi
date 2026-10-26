import assert from "node:assert/strict";
import { JSDOM } from "jsdom";
import { mkdir, writeFile } from "node:fs/promises";
function inspect(html, headers, language, expectProduct = false) {
  const document = new JSDOM(html).window.document;
  assert.equal(document.documentElement.lang, language);
  assert.equal(document.documentElement.dir, language === "ar" ? "rtl" : "ltr");
  assert.match(headers.get("x-robots-tag") ?? "", /noindex/);
  assert.match(headers.get("cache-control") ?? "", /no-store/);
  assert.match(document.body.textContent ?? "", /Test fixtures only/);
  assert.equal(
    document.querySelectorAll('link[rel="alternate"][hreflang]').length,
    0,
    "Draft locales cannot advertise indexable equivalents",
  );
  assert.equal(
    document.querySelectorAll('script[type="application/ld+json"]').length,
    0,
    "Fixtures cannot advertise merchant offers",
  );
  if (expectProduct) {
    assert.match(document.body.textContent ?? "", /DEMO-COURTYARD-COVERS/);
    assert.match(document.body.textContent ?? "", /EUR 123.45/);
    assert.match(document.body.textContent ?? "", /covers-only/);
  }
}
const base = process.env.WEB_BASE_URL;
if (!base || !process.env.PREVIEW_TOKEN)
  throw new Error("Owned app fixture required");
const authorization = `Basic ${Buffer.from(`preview:${process.env.PREVIEW_TOKEN}`).toString("base64")}`;
const fetchHtml = async (path, agent) => {
  const response = await fetch(base + path, {
    headers: { Authorization: authorization, "User-Agent": agent },
    redirect: "manual",
  });
  assert.equal(response.status, 200, `Missing expected route ${path}`);
  return { html: await response.text(), headers: response.headers };
};
let measured = 0;
for (const agent of ["Googlebot", "HTML-limited-crawler"]) {
  for (const locale of ["nl", "de", "fr", "en", "ar", "tr", "ku"]) {
    for (const suffix of ["", "/products/courtyard-demo"]) {
      const result = await fetchHtml(`/${locale}${suffix}`, agent);
      inspect(
        result.html,
        result.headers,
        locale === "ku" ? "ku-Latn" : locale,
        Boolean(suffix),
      );
      measured++;
    }
  }
}
const sample = await fetchHtml(
  "/nl/products/courtyard-demo",
  "HTML-limited-crawler",
);
for (const corrupt of [
  sample.html.replaceAll("DEMO-COURTYARD-COVERS", "missing"),
  sample.html.replace('lang="nl"', 'lang="de"'),
  sample.html.replace(
    "</head>",
    '<link rel="alternate" hreflang="ar" href="/ar"></head>',
  ),
  sample.html.replace(
    "</head>",
    '<script type="application/ld+json">{"@type":"Offer"}</script></head>',
  ),
])
  assert.throws(() => inspect(corrupt, sample.headers, "nl", true));
const wrongHeaders = new Headers(sample.headers);
wrongHeaders.delete("x-robots-tag");
assert.throws(() => inspect(sample.html, wrongHeaders, "nl", true));
const sitemap = await fetchHtml("/sitemap.xml", "Googlebot");
assert.equal(
  new JSDOM(sitemap.html, {
    contentType: "text/xml",
  }).window.document.querySelectorAll("url").length,
  0,
);
const robots = await fetchHtml("/robots.txt", "Googlebot");
assert.match(robots.html, /Allow: \//);
const anonymous = await fetch(`${base}/nl`);
assert.equal(anonymous.status, 401);
const forged = await fetch(`${base}/nl`, {
  headers: { Authorization: authorization, Host: "forged.invalid" },
});
const forgedHtml = await forged.text();
assert.equal(forgedHtml.includes('href="http://forged.invalid'), false);
await mkdir(".local/evidence", { recursive: true });
await writeFile(
  ".local/evidence/discovery.json",
  JSON.stringify(
    {
      stage: "private-preview",
      servedCases: measured,
      negativeControls: 5,
      indexableRoutes: 0,
      indexing: "not-measured",
    },
    null,
    2,
  ),
);
console.log(
  `Discovery: ${measured} served HTML cases and hostile controls passed; no public indexable catalogue.`,
);
