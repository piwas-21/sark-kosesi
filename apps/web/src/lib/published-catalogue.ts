import type { components } from "@piwas/contracts";
import { isLocale } from "./locale";
import {
  record,
  text,
  integer,
  merchantCode,
  slug,
  keys,
  measurements,
  decimal,
} from "./published-validation";
export type PublishedProduct = Omit<
  components["schemas"]["PublishedProduct"],
  "images"
> & {
  images: [
    components["schemas"]["ProductImage"],
    ...components["schemas"]["ProductImage"][],
  ];
};
export type PublishedCatalogue = Omit<
  components["schemas"]["PublishedCatalogue"],
  "next_after" | "items"
> & { next_after: string | null; items: PublishedProduct[] };
function copy(value: unknown): boolean {
  return (
    record(value) &&
    keys(value, [
      "title",
      "description",
      "materials",
      "care",
      "safety",
      "assembly",
    ]) &&
    text(value.title, 160) &&
    text(value.description, 4000) &&
    [value.materials, value.care, value.safety, value.assembly].every((v) =>
      text(v, 2000),
    )
  );
}
function material(value: unknown): boolean {
  if (!record(value)) return false;
  if (value.kind === "foam")
    return (
      keys(value, ["kind", "density_g_per_m3"]) &&
      integer(value.density_g_per_m3, 1, 1000000)
    );
  return (
    value.kind === "cover" &&
    keys(value, ["kind", "fibres"]) &&
    Array.isArray(value.fibres) &&
    value.fibres.length >= 1 &&
    value.fibres.length <= 16 &&
    value.fibres.every(
      (v: unknown) =>
        record(v) &&
        keys(v, ["name", "basis_points"]) &&
        text(v.name, 80) &&
        integer(v.basis_points, 1, 10000),
    )
  );
}
function product(value: unknown): value is PublishedProduct {
  if (
    !record(value) ||
    !keys(value, [
      "sku",
      "locale",
      "publication_revision",
      "family_code",
      "family_slug",
      "size_code",
      "textile_code",
      "fill",
      "minor_units",
      "currency",
      "price_decimal",
      "copy",
      "footprint",
      "components",
      "parcels",
      "images",
      "availability",
    ]) ||
    !merchantCode(value.sku) ||
    typeof value.locale !== "string" ||
    !isLocale(value.locale) ||
    !integer(value.publication_revision, 1, 2147483647) ||
    !merchantCode(value.family_code) ||
    !slug(value.family_slug) ||
    !text(value.size_code, 80) ||
    !text(value.textile_code, 80) ||
    (value.fill !== "covers-only" && value.fill !== "with-foam") ||
    !integer(value.minor_units, 0, Number.MAX_SAFE_INTEGER) ||
    value.currency !== "EUR" ||
    value.price_decimal !== decimal(value.minor_units) ||
    !copy(value.copy) ||
    !measurements(value.footprint) ||
    (value.availability !== "in_stock" && value.availability !== "out_of_stock")
  )
    return false;
  if (
    !Array.isArray(value.components) ||
    value.components.length < 1 ||
    value.components.length > 64 ||
    !value.components.every(
      (v: unknown) =>
        record(v) &&
        keys(v, ["code", "quantity", "dimensions", "material"]) &&
        merchantCode(v.code) &&
        integer(v.quantity, 1, 10000) &&
        measurements(v.dimensions) &&
        material(v.material),
    )
  )
    return false;
  if (
    !Array.isArray(value.parcels) ||
    value.parcels.length < 1 ||
    value.parcels.length > 64 ||
    !value.parcels.every(
      (v: unknown) =>
        record(v) &&
        keys(v, ["dimensions", "gross_weight_g"]) &&
        measurements(v.dimensions) &&
        integer(v.gross_weight_g, 1, 1000000),
    )
  )
    return false;
  return (
    Array.isArray(value.images) &&
    value.images.length >= 1 &&
    value.images.length <= 3 &&
    value.images.every(
      (v: unknown) =>
        record(v) &&
        keys(v, ["url", "edge", "width", "height"]) &&
        integer(v.edge, 320, 960) &&
        [320, 640, 960].includes(v.edge) &&
        integer(v.width, 1, 960) &&
        integer(v.height, 1, 960) &&
        v.url ===
          `/api/published/${value.locale}/images/${value.sku}/${value.publication_revision}/${v.edge}`,
    )
  );
}
export function decodePublishedProduct(value: unknown): PublishedProduct {
  if (!product(value)) throw new Error("Invalid published product");
  return value;
}
export function decodePublishedCatalogue(value: unknown): PublishedCatalogue {
  if (
    !record(value) ||
    !keys(value, ["items", "locale", "next_after", "production_enabled"]) ||
    value.production_enabled !== false ||
    typeof value.locale !== "string" ||
    !isLocale(value.locale) ||
    (value.next_after !== null && !merchantCode(value.next_after)) ||
    !Array.isArray(value.items) ||
    value.items.length > 25
  )
    throw new Error("Invalid published catalogue");
  const items = value.items.map(decodePublishedProduct);
  if (
    items.some(
      (p, index) =>
        p.locale !== value.locale ||
        (index > 0 && (items[index - 1]?.sku ?? "") >= p.sku) ||
        (typeof value.next_after === "string" && p.sku > value.next_after),
    )
  )
    throw new Error("Invalid published page identity");
  return {
    items,
    locale: value.locale,
    next_after: value.next_after,
    production_enabled: false,
  };
}
