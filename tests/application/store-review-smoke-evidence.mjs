import { constants } from "node:fs";
import { open } from "node:fs/promises";

export class CheckFailure extends Error {}

export function requireCheck(condition, label) {
  if (!condition) throw new CheckFailure(label);
}

export const policySections = {
  delivery: "bezorging",
  returns: "retourneren",
  privacy: "privacy",
  payment: "betalen",
  terms: "voorwaarden",
};

const evidenceLimit = 64 * 1024;
const merchantCode = /^[A-Z0-9](?:[A-Z0-9-]{0,63}[A-Z0-9])?$/;
const sha256 = /^[a-f0-9]{64}$/;
const imageRoute =
  /^\/catalogue-images\/nl\/([A-Z0-9-]+)\/([1-9]\d{0,9})\/(320|640|960)$/;

function record(value, fields) {
  return (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    Object.keys(value).length === fields.length &&
    fields.every((field) => Object.hasOwn(value, field))
  );
}

function text(value, limit) {
  return (
    typeof value === "string" &&
    value.trim().length > 0 &&
    Array.from(value).length <= limit &&
    !value.includes("\0")
  );
}

function matches(pattern, value) {
  return typeof value === "string" && pattern.test(value);
}

function validStore(store) {
  return (
    record(store, [
      "storeName",
      "sellerName",
      "kvk",
      "vatId",
      "address",
      "email",
      "phone",
      "policies",
    ]) &&
    text(store.storeName, 80) &&
    text(store.sellerName, 120) &&
    text(store.address, 420) &&
    matches(/^\d{8}$/, store.kvk) &&
    matches(/^NL\d{9}B\d{2}$/, store.vatId) &&
    text(store.email, 254) &&
    matches(/^[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}$/i, store.email) &&
    matches(/^\+[1-9]\d{6,14}$/, store.phone) &&
    record(store.policies, Object.keys(policySections)) &&
    Object.values(store.policies).every((policy) => text(policy, 8000))
  );
}

function validProduct(product) {
  if (
    !record(product, [
      "slug",
      "sku",
      "title",
      "price",
      "fill",
      "imagePath",
      "imageSha256",
    ])
  )
    return false;
  const image =
    typeof product.imagePath === "string" && imageRoute.exec(product.imagePath);
  return (
    matches(merchantCode, product.sku) &&
    product.sku !== "DEMO" &&
    !product.sku.startsWith("DEMO-") &&
    text(product.slug, 100) &&
    matches(/^[a-z0-9]+(?:-[a-z0-9]+)*$/, product.slug) &&
    text(product.title, 300) &&
    matches(/^(?:0|[1-9]\d{0,13})\.\d{2}$/, product.price) &&
    Number.isSafeInteger(Number(product.price.replace(".", ""))) &&
    ["with-foam", "covers-only"].includes(product.fill) &&
    image &&
    image[1] === product.sku &&
    Number(image[2]) <= 2147483647 &&
    matches(sha256, product.imageSha256)
  );
}

export async function loadEvidence(path) {
  const file = await open(
    path,
    constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK,
  );
  let bytes;
  try {
    const metadata = await file.stat();
    requireCheck(
      metadata.isFile() &&
        metadata.size <= evidenceLimit &&
        (metadata.mode & 0o077) === 0,
      "Evidence file must be private, regular and bounded",
    );
    const buffer = Buffer.alloc(evidenceLimit + 1);
    let length = 0;
    while (length < buffer.length) {
      const { bytesRead } = await file.read(
        buffer,
        length,
        buffer.length - length,
      );
      if (bytesRead === 0) break;
      length += bytesRead;
    }
    requireCheck(length <= evidenceLimit, "Evidence file exceeds size limit");
    bytes = buffer.subarray(0, length);
  } finally {
    await file.close();
  }

  let evidence;
  try {
    evidence = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    );
  } catch {
    throw new CheckFailure("Evidence file must contain valid UTF-8 JSON");
  }
  requireCheck(
    record(evidence, ["store", "products"]) &&
      validStore(evidence.store) &&
      Array.isArray(evidence.products) &&
      evidence.products.length >= 1 &&
      evidence.products.length <= 40 &&
      evidence.products.every(validProduct) &&
      new Set(evidence.products.map(({ sku }) => sku)).size ===
        evidence.products.length,
    "Evidence must contain exact seller, policies and distinct approved products",
  );
  return evidence;
}

export function productHref({ slug, sku }) {
  return `/nl/collection/${slug}?variant=${sku}`;
}
