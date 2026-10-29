import type { PublishedProduct } from "../src/lib/published-catalogue";
export const dimensions = {
  width_mm: 500,
  depth_mm: 500,
  height_mm: 100,
  tolerance_mm: 5,
};
export const approved: PublishedProduct = {
  sku: "SET-COVERS",
  locale: "nl",
  publication_revision: 1,
  family_code: "ROOM",
  family_slug: "isolated-room",
  size_code: "Isolated size",
  textile_code: "Isolated textile",
  fill: "covers-only",
  minor_units: 12345,
  currency: "EUR",
  price_decimal: "123.45",
  copy: {
    title: "Synthetic acceptance sample",
    description: "Test fixture <script>never executable</script>",
    materials: "Synthetic fibres",
    care: "Test care",
    safety: "Test safety",
    assembly: "Test assembly",
  },
  footprint: { ...dimensions, width_mm: 1000 },
  components: [
    {
      code: "SEAT-COVER",
      quantity: 2,
      dimensions,
      material: {
        kind: "cover",
        fibres: [{ name: "Synthetic fibre", basis_points: 10000 }],
      },
    },
  ],
  parcels: [{ dimensions, gross_weight_g: 3000 }],
  images: [
    {
      edge: 320,
      width: 32,
      height: 24,
      url: "/api/published/nl/images/SET-COVERS/1/320",
    },
  ],
  availability: "in_stock",
};
export const page = (
  items: PublishedProduct[] = [approved],
  next_after: string | null = null,
) => ({ items, locale: "nl", next_after, production_enabled: false });
