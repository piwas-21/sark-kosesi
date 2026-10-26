import type { components } from "@piwas/contracts";
export type Catalogue = components["schemas"]["Catalogue"];
export type Product = components["schemas"]["Product"];
function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function text(value: unknown, maximum: number): value is string {
  return typeof value === "string" && value.length <= maximum;
}
export function decodeCatalogue(value: unknown): Catalogue {
  if (
    !record(value) ||
    value.production_enabled !== false ||
    !text(value.locale, 3) ||
    !Array.isArray(value.items) ||
    value.items.length > 50
  )
    throw new Error("Invalid catalogue response");
  const items: Product[] = value.items.map((item: unknown) => {
    if (
      !record(item) ||
      !text(item.sku, 65) ||
      !/^DEMO-[A-Z0-9-]+$/.test(item.sku) ||
      !text(item.slug, 100) ||
      !/^[a-z0-9]+(-[a-z0-9]+)*$/.test(item.slug) ||
      !text(item.title, 160) ||
      !text(item.description, 2000) ||
      !text(item.care, 2000) ||
      typeof item.minor_units !== "number" ||
      !Number.isSafeInteger(item.minor_units) ||
      item.minor_units < 0 ||
      item.currency !== "EUR" ||
      !["covers-only", "with-foam"].includes(String(item.fill)) ||
      typeof item.fill !== "string" ||
      item.fixture !== true
    )
      throw new Error("Invalid catalogue item");
    return {
      sku: item.sku,
      slug: item.slug,
      title: item.title,
      description: item.description,
      care: item.care,
      minor_units: item.minor_units,
      currency: item.currency,
      fill: item.fill,
      fixture: item.fixture,
    };
  });
  return { items, locale: value.locale, production_enabled: false };
}
