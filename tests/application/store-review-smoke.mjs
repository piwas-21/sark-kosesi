// Anonymous GET/HEAD readback for the dedicated Dutch, non-selling storefront.
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import {
  CheckFailure,
  loadEvidence,
  productHref,
  requireCheck,
} from "./store-review-smoke-evidence.mjs";
import {
  collectionPath,
  informationPath,
  inspectCollection,
  inspectInformation,
  inspectPage,
  inspectProduct,
} from "./store-review-smoke-html.mjs";

const deniedPaths = [
  "/nl/admin",
  "/api/catalogue/nl",
  "/nl/catalogue.xml",
  "/nl/room-planner",
  "/en",
  "/sitemap.xml",
];
const htmlLimit = 2 * 1024 * 1024;
const imageLimit = 250 * 1024;
const nextCollection =
  /^\/nl\/collection\?after=[A-Z0-9](?:[A-Z0-9-]{0,63}[A-Z0-9])?$/;
const pageLimit = 40;

function directives(response, name) {
  return (response.headers.get(name) ?? "")
    .toLowerCase()
    .split(",")
    .map((part) => part.trim());
}

function verifyHeaders(response, status, revision, label) {
  requireCheck(response.status === status, `${label}: unexpected HTTP status`);
  requireCheck(
    response.headers.get("x-piwas-preview-revision") === revision,
    `${label}: wrong served revision`,
  );
  const cache = directives(response, "cache-control");
  const robots = directives(response, "x-robots-tag");
  requireCheck(
    cache.includes("private") &&
      cache.includes("no-store") &&
      !cache.includes("public") &&
      robots.includes("noindex") &&
      robots.includes("nofollow") &&
      !robots.includes("index") &&
      !robots.includes("follow"),
    `${label}: missing or conflicting cache or indexing directives`,
  );
  requireCheck(
    response.headers.get("x-content-type-options") === "nosniff" &&
      response.headers.get("referrer-policy") === "no-referrer" &&
      !response.headers.has("www-authenticate"),
    `${label}: unexpected anonymous response headers`,
  );
}

async function readBody(response, maximum, label) {
  requireCheck(response.body, `${label}: missing response body`);
  const chunks = [];
  let bytes = 0;
  for await (const chunk of response.body) {
    bytes += chunk.byteLength;
    requireCheck(bytes <= maximum, `${label}: response body exceeds limit`);
    chunks.push(chunk);
  }
  requireCheck(bytes > 0, `${label}: empty response body`);
  return Buffer.concat(chunks, bytes);
}

function requestAt({ connectionOrigin, revision, request }) {
  return async (method, path, status, label) => {
    let response;
    try {
      response = await request(new URL(path, connectionOrigin), {
        method,
        redirect: "manual",
        headers: {
          "Accept-Language": "en",
          "User-Agent": "Piwas-store-review-smoke",
        },
        signal: AbortSignal.timeout(10_000),
      });
    } catch {
      throw new CheckFailure(`${label}: request failed`);
    }
    try {
      verifyHeaders(response, status, revision, label);
    } catch (error) {
      await response.body?.cancel();
      throw error;
    }
    return response;
  };
}

async function verifyRedirect(probe, path, target, origin, label) {
  for (const method of ["GET", "HEAD"]) {
    const response = await probe(method, path, 307, label);
    const location = response.headers.get("location");
    await response.body?.cancel();
    requireCheck(
      location && new URL(location, origin).href === origin + target,
      `${label}: wrong canonical redirect`,
    );
  }
}

async function verifyPage(probe, path, label, inspect) {
  const response = await probe("GET", path, 200, label);
  const html = await readBody(response, htmlLimit, label);
  const head = await probe("HEAD", path, 200, label);
  for (const receipt of [response, head]) {
    requireCheck(
      /^text\/html(?:;|$)/i.test(receipt.headers.get("content-type") ?? ""),
      `${label}: expected HTML response`,
    );
  }
  return inspectPage(
    new TextDecoder("utf-8", { fatal: true }).decode(html),
    inspect,
  );
}

async function verifyImage(probe, product) {
  const label = "Approved image";
  const response = await probe("GET", product.imagePath, 200, label);
  const bytes = await readBody(response, imageLimit, label);
  const head = await probe("HEAD", product.imagePath, 200, label);
  const mime = response.headers.get("content-type");
  const signature = mime === "image/jpeg" ? "ffd8ff" : "89504e470d0a1a0a";
  requireCheck(
    ["image/jpeg", "image/png"].includes(mime) &&
      head.headers.get("content-type") === mime &&
      bytes.subarray(0, signature.length / 2).toString("hex") === signature &&
      createHash("sha256").update(bytes).digest("hex") === product.imageSha256,
    "Served rendition differs from approved image evidence",
  );
}

export async function runStoreReviewSmoke({
  origin,
  connectionOrigin,
  revision,
  evidence,
  request = fetch,
}) {
  const probe = requestAt({ connectionOrigin, revision, request });
  await verifyRedirect(
    probe,
    "/?variant=discarded",
    "/nl",
    origin,
    "Store entry",
  );
  await verifyRedirect(probe, "/nl", collectionPath, origin, "Dutch home");

  const products = new Map(
    evidence.products.map((product) => [productHref(product), product]),
  );
  const seen = new Set();
  const visited = new Set();
  let path = collectionPath;
  while (path !== null) {
    requireCheck(
      visited.size < pageLimit && !visited.has(path),
      "Review collection pagination did not terminate",
    );
    visited.add(path);
    path = await verifyPage(probe, path, "Review collection", (doc) =>
      inspectCollection(doc, products, seen),
    );
    requireCheck(
      path === null || nextCollection.test(path),
      "Review collection pagination must stay on the Dutch collection",
    );
  }
  requireCheck(
    seen.size === products.size,
    "Review collection omits approved products",
  );

  for (const [path, product] of products) {
    await verifyPage(probe, path, "Product detail", (doc) =>
      inspectProduct(doc, product, origin),
    );
    await verifyImage(probe, product);
  }
  await verifyPage(probe, informationPath, "Store information", (doc) =>
    inspectInformation(doc, evidence.store),
  );
  for (const path of deniedPaths) {
    for (const method of ["GET", "HEAD"]) {
      const response = await probe(method, path, 404, "Restricted route");
      await response.body?.cancel();
    }
  }
  return {
    origin,
    revision,
    transport: connectionOrigin === origin ? "https" : "private-loopback",
    approvedProducts: seen.size,
    collectionPages: visited.size,
  };
}

async function main() {
  let values;
  try {
    ({ values } = parseArgs({
      options: {
        origin: { type: "string" },
        revision: { type: "string" },
        "evidence-file": { type: "string" },
        "candidate-port": { type: "string" },
      },
    }));
  } catch {
    throw new CheckFailure("Invalid command arguments");
  }
  const {
    origin,
    revision,
    "evidence-file": evidenceFile,
    "candidate-port": candidatePort,
  } = values;
  requireCheck(
    origin && /^[a-f0-9]{40}$/.test(revision ?? "") && evidenceFile,
    "Supply origin, revision and evidence file",
  );
  const canonical = URL.parse(origin);
  requireCheck(
    canonical?.protocol === "https:" && canonical.origin === origin,
    "Supply a bare HTTPS storefront origin",
  );
  requireCheck(
    candidatePort === undefined ||
      (/^[1-9]\d{0,4}$/.test(candidatePort) && Number(candidatePort) <= 65535),
    "Candidate port must be a valid loopback port",
  );
  const connectionOrigin =
    candidatePort === undefined ? origin : `http://127.0.0.1:${candidatePort}`;
  requireCheck(
    candidatePort !== undefined ||
      process.env.NODE_TLS_REJECT_UNAUTHORIZED !== "0",
    "HTTPS readback requires TLS verification",
  );
  const evidence = await loadEvidence(evidenceFile);
  const result = await runStoreReviewSmoke({
    origin,
    connectionOrigin,
    revision,
    evidence,
  });
  console.log(JSON.stringify(result));
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  try {
    await main();
  } catch (error) {
    const message =
      error instanceof CheckFailure ? error.message : "Readback failed";
    console.error(`Store review smoke failed: ${message}`);
    process.exitCode = 1;
  }
}
