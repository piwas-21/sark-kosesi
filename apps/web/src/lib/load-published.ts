import "server-only";
import createClient from "openapi-fetch";
import type { paths } from "@piwas/contracts";
import { configuration } from "./config";
import {
  decodePublishedCatalogue,
  decodePublishedProduct,
  type PublishedProduct,
} from "./published-catalogue";
import { merchantCode } from "./published-validation";
import type { Locale } from "./locale";
function client() {
  const { api, token } = configuration();
  return createClient<paths>({
    baseUrl: api,
    headers: { Authorization: `Bearer ${token}` },
  });
}
export async function publishedPage(locale: Locale, after?: string) {
  const { data, response } = await client().GET("/api/published/{locale}", {
    params: { path: { locale }, query: after === undefined ? {} : { after } },
    cache: "no-store",
    signal: AbortSignal.timeout(1500),
  });
  if (!response.ok) throw new Error("Published catalogue unavailable");
  const page = decodePublishedCatalogue(data);
  if (
    page.locale !== locale ||
    page.items.some((p) => after !== undefined && p.sku <= after) ||
    (page.next_after !== null &&
      after !== undefined &&
      page.next_after <= after)
  )
    throw new Error("Published cursor mismatch");
  return page;
}
export async function publishedProduct(locale: Locale, sku: string) {
  if (!merchantCode(sku)) return null;
  const { data, response } = await client().GET(
    "/api/published/{locale}/products/{sku}",
    {
      params: { path: { locale, sku } },
      cache: "no-store",
      signal: AbortSignal.timeout(1500),
    },
  );
  if (response.status === 404) return null;
  if (!response.ok) throw new Error("Published product unavailable");
  const product = decodePublishedProduct(data);
  if (product.locale !== locale || product.sku !== sku)
    throw new Error("Published product identity mismatch");
  return product;
}
export async function publishedCollection(locale: Locale) {
  const items: PublishedProduct[] = [];
  let after: string | undefined;
  // A bounded preview fails as a whole instead of emitting a silently partial feed.
  for (let pageNumber = 0; pageNumber < 40; pageNumber++) {
    const page = await publishedPage(locale, after);
    items.push(...page.items);
    if (page.next_after === null) return items;
    after = page.next_after;
  }
  throw new Error("Published collection exceeds preview limit");
}
