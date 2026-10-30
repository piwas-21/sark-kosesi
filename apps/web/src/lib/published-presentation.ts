import { type Locale } from "./locale";
import type { PublishedProduct } from "./published-catalogue";
export { displayPrice } from "./published-price";
export function productPath(product: PublishedProduct): string {
  return `/${product.locale}/collection/${product.family_slug}?variant=${encodeURIComponent(product.sku)}`;
}
export function imagePath(url: string): string {
  if (
    !/^\/api\/published\/(nl|de|fr|en|ar|tr|ku)\/images\/[A-Z0-9-]+\/[1-9]\d*\/(320|640|960)$/.test(
      url,
    )
  )
    throw new Error("Invalid image URL");
  return url.replace(
    /^\/api\/published\/([^/]+)\/images\//,
    "/catalogue-images/$1/",
  );
}
export function xml(value: string): string {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (!(
      code === 9 ||
      code === 10 ||
      code === 13 ||
      (code >= 32 && code <= 0xd7ff) ||
      (code >= 0xe000 && code <= 0xfffd) ||
      (code >= 0x10000 && code <= 0x10ffff)
    ))
      throw new Error("Copy is not representable in XML");
  }
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&apos;");
}
export function catalogueFeed(
  items: PublishedProduct[],
  origin: string,
  locale: Locale,
): string {
  return `<?xml version="1.0" encoding="UTF-8"?><catalogue locale="${locale}" preview-only="true" production-enabled="false">${items
    .map((p) => {
      const image = p.images[0];

      return `<product><sku>${xml(p.sku)}</sku><title>${xml(p.copy.title)}</title><description>${xml(p.copy.description)}</description><link>${xml(origin + productPath(p))}</link><price currency="EUR">${p.price_decimal}</price><availability>${p.availability}</availability><fill>${p.fill}</fill><publication-revision>${p.publication_revision}</publication-revision><image>${xml(origin + imagePath(image.url))}</image></product>`;
    })
    .join("")}</catalogue>`;
}
