import { JSDOM } from "jsdom";
import {
  policySections,
  productHref,
  requireCheck,
} from "./store-review-smoke-evidence.mjs";

export const collectionPath = "/nl/collection";
export const informationPath = "/nl/winkelinformatie";
const unavailable = "Online bestellen is nog niet beschikbaar.";
const grossEuro = new Intl.NumberFormat("nl", {
  style: "currency",
  currency: "EUR",
});

function normalized(text) {
  return (text ?? "").normalize("NFC").replace(/\s+/gu, " ").trim();
}

function contents(element, selector) {
  return normalized(element.querySelector(selector)?.textContent);
}

function elements(parent, selector) {
  return [...parent.querySelectorAll(selector)];
}

function expectedPrice(price) {
  return normalized(grossEuro.format(price));
}

function hasGrossPrice(element, price) {
  if (!element) return false;
  const directText = [...element.childNodes]
    .filter((node) => node.nodeType === 3)
    .map((node) => node.textContent)
    .join("");
  return (
    normalized(directText) === expectedPrice(price) &&
    contents(element, "span") === "incl. btw"
  );
}

function fillLabel(fill) {
  return fill === "with-foam"
    ? "Met schuimvulling"
    : "Alleen hoezen, zonder schuim";
}

export function inspectPage(html, inspect) {
  const { window } = new JSDOM(html);
  try {
    const doc = window.document;
    for (const element of elements(
      doc,
      "script:not([type='application/ld+json']), template, [hidden]",
    ))
      element.remove();
    requireCheck(
      doc.documentElement.lang === "nl" && doc.documentElement.dir === "ltr",
      "Expected Dutch review HTML",
    );
    requireCheck(
      !doc.querySelector(
        'form, script[type="application/ld+json"], link[hreflang]',
      ),
      "Review pages must omit commerce forms, structured offers and alternate locales",
    );
    requireCheck(
      !elements(doc, "a[href], button").some(
        (element) =>
          /^(?:koop nu|bestellen|in winkelwagen|toevoegen aan winkelwagen|afrekenen)$/iu.test(
            normalized(element.textContent),
          ) ||
          /^\/(?:[a-z]{2}\/)?(?:checkout|cart|basket|winkelwagen|afrekenen)(?:[/?#]|$)/i.test(
            element.getAttribute("href") ?? "",
          ),
      ),
      "Review pages must omit purchase controls",
    );
    requireCheck(
      doc.querySelector(
        'nav[aria-label="Hoofdnavigatie"] a[href="/nl/collection"]',
      ) &&
        doc.querySelector(
          'nav[aria-label="Hoofdnavigatie"] a[href="/nl/winkelinformatie"]',
        ),
      "Expected collection and policy navigation",
    );
    return inspect(doc);
  } finally {
    window.close();
  }
}

export function inspectCollection(doc, expected, seen) {
  requireCheck(
    contents(doc, "main h1") === "De collectie",
    "Expected review collection heading",
  );
  requireCheck(
    normalized(doc.querySelector("main")?.textContent).includes(unavailable),
    "Expected review-only collection status",
  );
  const cards = elements(doc, "main article");
  requireCheck(
    cards.length > 0,
    "Review collection must contain approved products",
  );
  for (const card of cards) {
    const href = card.querySelector("h2 a")?.getAttribute("href");
    const product = expected.get(href);
    requireCheck(
      product && !seen.has(href),
      "Unexpected or duplicate collection product",
    );
    requireCheck(
      contents(card, "h2") === normalized(product.title) &&
        contents(card, "bdi") === product.sku &&
        card.querySelector("img")?.getAttribute("src") === product.imagePath &&
        elements(card, "p").some(
          (p) => normalized(p.textContent) === fillLabel(product.fill),
        ) &&
        elements(card, "p").some((p) => hasGrossPrice(p, product.price)),
      "Collection card differs from approved product evidence",
    );
    seen.add(href);
  }
  const next = elements(doc, 'main a[href^="/nl/collection?"]');
  requireCheck(next.length <= 1, "Invalid review collection pagination");
  return next[0]?.getAttribute("href") ?? null;
}

export function inspectProduct(doc, product, canonicalOrigin) {
  const item = doc.querySelector("main article");
  requireCheck(
    item &&
      item.getAttribute("data-selected-sku") === product.sku &&
      contents(item, "h1") === normalized(product.title) &&
      item.querySelector('[aria-current="true"]')?.getAttribute("href") ===
        productHref(product),
    "Selected product differs from approved SKU and title",
  );
  requireCheck(
    item.querySelector("[data-price]")?.getAttribute("data-price") ===
      product.price &&
      hasGrossPrice(item.querySelector("[data-price]"), product.price),
    "Rendered gross product price differs from approved price",
  );
  requireCheck(
    item.querySelector("img")?.getAttribute("src") === product.imagePath &&
      normalized(
        item.querySelector('a[aria-current="true"]')?.textContent,
      ).startsWith(fillLabel(product.fill)) &&
      normalized(item.textContent).includes(unavailable),
    "Expected approved image, fill and review-only product status",
  );
  requireCheck(
    doc.querySelector('link[rel="canonical"]')?.getAttribute("href") ===
      `${canonicalOrigin}${collectionPath}/${product.slug}`,
    "Product canonical must use the exact storefront origin",
  );
}

export function inspectInformation(doc, store) {
  const address = elements(doc, "main address p").map((line) =>
    normalized(line.textContent),
  );
  const identity = elements(doc, "main dd").map((detail) =>
    normalized(detail.textContent),
  );
  requireCheck(
    normalized(doc.title) === normalized(store.storeName) &&
      contents(doc, "main h1") === "Winkelinformatie" &&
      address.length === 4 &&
      address[0] === normalized(store.sellerName) &&
      address[1] === normalized(store.address) &&
      address[2] === store.email &&
      address[3] === store.phone &&
      doc
        .querySelector('main address a[href^="mailto:"]')
        ?.getAttribute("href") === `mailto:${store.email}` &&
      doc
        .querySelector('main address a[href^="tel:"]')
        ?.getAttribute("href") === `tel:${store.phone}` &&
      identity.length === 2 &&
      identity[0] === store.kvk &&
      identity[1] === store.vatId,
    "Rendered seller identity differs from approved evidence",
  );
  for (const [policy, id] of Object.entries(policySections)) {
    requireCheck(
      contents(doc, `main section#${id} p`) ===
        normalized(store.policies[policy]) &&
        doc.querySelector(`footer a[href="${informationPath}#${id}"]`),
      `Rendered ${policy} policy or navigation differs from approved evidence`,
    );
  }
}
