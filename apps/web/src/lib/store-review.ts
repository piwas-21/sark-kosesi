import type { PublishedProduct } from "./published-catalogue";
import { keys, record, text } from "./published-validation";

const sections = [
  "delivery",
  "returns",
  "privacy",
  "payment",
  "terms",
] as const;
type PolicySection = (typeof sections)[number];

export type StoreReview = {
  storeName: string;
  sellerName: string;
  kvk: string;
  vatId: string;
  address: string;
  email: string;
  phone: string;
  policies: Record<PolicySection, string>;
};

function validPolicies(value: unknown): value is StoreReview["policies"] {
  return (
    record(value) &&
    keys(value, [...sections]) &&
    sections.every((section) => text(value[section], 8000))
  );
}

function validContent(value: unknown): value is StoreReview {
  return (
    record(value) &&
    keys(value, [
      "storeName",
      "sellerName",
      "kvk",
      "vatId",
      "address",
      "email",
      "phone",
      "policies",
    ]) &&
    text(value.storeName, 80) &&
    text(value.sellerName, 120) &&
    text(value.address, 420) &&
    typeof value.kvk === "string" &&
    /^\d{8}$/.test(value.kvk) &&
    typeof value.vatId === "string" &&
    /^NL\d{9}B\d{2}$/.test(value.vatId) &&
    typeof value.email === "string" &&
    value.email.length <= 254 &&
    /^[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}$/i.test(value.email) &&
    typeof value.phone === "string" &&
    /^\+[1-9]\d{6,14}$/.test(value.phone) &&
    validPolicies(value.policies)
  );
}

export function decodeStoreReview(content: string): StoreReview {
  if (Buffer.byteLength(content) > 48_000)
    throw new Error("Invalid store review content");
  let value: unknown;
  try {
    value = JSON.parse(content);
  } catch {
    throw new Error("Invalid store review content");
  }
  if (!validContent(value)) throw new Error("Invalid store review content");
  return value;
}

export const storeInformationPath = "/nl/winkelinformatie";

export function storeReviewFill(fill: PublishedProduct["fill"]): string {
  return fill === "with-foam"
    ? "Met schuimvulling"
    : "Alleen hoezen, zonder schuim";
}
