// Read-only acceptance against the dedicated preview. No provider/order mutations.
import assert from "node:assert/strict";
import { readFile, stat } from "node:fs/promises";
import { parseArgs } from "node:util";
const { values } = parseArgs({
  options: {
    origin: { type: "string" },
    revision: { type: "string" },
    "token-file": { type: "string" },
  },
});
assert(
  values.origin && values.revision && values["token-file"],
  "Supply origin, revision and private token-file",
);
const origin = new URL(values.origin);
assert(
  origin.protocol === "https:" &&
    origin.origin === values.origin &&
    !origin.username &&
    !origin.password,
  "Supply a bare HTTPS origin",
);
assert(
  /^[a-f0-9]{40}$/.test(values.revision),
  "Supply the exact reviewed revision",
);
const metadata = await stat(values["token-file"]);
assert(
  metadata.isFile() && metadata.size <= 129 && (metadata.mode & 0o077) === 0,
  "Token file must be private and bounded",
);
const token = (await readFile(values["token-file"], "utf8")).trim();
assert(
  /^[A-Za-z0-9_-]{32,128}$/.test(token),
  "Invalid preview credential format",
);
const authorization = `Basic ${Buffer.from(`preview:${token}`).toString("base64")}`;
async function get(path, headers = {}) {
  return fetch(new URL(path, origin), {
    headers,
    redirect: "manual",
    signal: AbortSignal.timeout(10000),
  });
}
function privateHeaders(response) {
  assert.equal(
    response.headers.get("x-piwas-preview-revision"),
    values.revision,
    "Wrong served source",
  );
  assert.match(
    response.headers.get("cache-control") ?? "",
    /private, no-store/,
  );
  assert.match(response.headers.get("x-robots-tag") ?? "", /noindex, nofollow/);
  assert.equal(response.headers.get("referrer-policy"), "no-referrer");
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
}
for (const headers of [{}, { authorization: "Basic invalid" }]) {
  const response = await get("/nl", headers);
  assert.equal(
    response.status,
    401,
    "Preview must reject anonymous/wrong credentials",
  );
  assert.match(response.headers.get("www-authenticate") ?? "", /Basic/);
  privateHeaders(response);
  await response.body?.cancel();
}
const redirected = await get("/?variant=preserved", {
  authorization,
  "accept-language": "ar",
});
assert.equal(redirected.status, 307);
assert.equal(
  redirected.headers.get("location"),
  `${origin.origin}/ar?variant=preserved`,
);
privateHeaders(redirected);
await redirected.body?.cancel();
for (const [locale, direction] of [
  ["nl", "ltr"],
  ["ar", "rtl"],
]) {
  const response = await get(`/${locale}`, { authorization });
  assert.equal(response.status, 200);
  privateHeaders(response);
  assert.match(
    response.headers.get("content-security-policy") ?? "",
    /frame-ancestors 'none'/,
  );
  const html = await response.text();
  assert.match(html, new RegExp(`<html[^>]*lang="${locale}"`));
  assert.match(html, new RegExp(`<html[^>]*dir="${direction}"`));
}
const catalogue = await get("/api/catalogue/nl", { authorization });
assert.equal(catalogue.status, 200);
privateHeaders(catalogue);
const body = await catalogue.json();
assert.equal(
  body.production_enabled,
  false,
  "Preview must report disabled production",
);
assert(
  Array.isArray(body.items),
  "Catalogue must be a genuine decoded response",
);
console.log(
  JSON.stringify({
    origin: origin.origin,
    revision: values.revision,
    anonymous: "blocked",
    wrongCredential: "blocked",
    localeRedirect: "same HTTPS origin",
    locales: ["nl", "ar"],
    productionEnabled: false,
    catalogueItems: body.items.length,
  }),
);
