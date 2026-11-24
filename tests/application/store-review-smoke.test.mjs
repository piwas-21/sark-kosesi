import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { loadEvidence } from "./store-review-smoke-evidence.mjs";
import { runStoreReviewSmoke } from "./store-review-smoke.mjs";

const exec = promisify(execFile);
const executable = fileURLToPath(
  new URL("./store-review-smoke.mjs", import.meta.url),
);
const origin = "https://review.example.test";
const revision = "abcdef0123456789abcdef0123456789abcdef01";
const connectionOrigin = "http://127.0.0.1:3000";
const photo = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO2n1uQAAAAASUVORK5CYII=",
  "base64",
);
const imagePath = "/catalogue-images/nl/SET-COVERS/4/320";
const productPath = "/nl/collection/test-collectie?variant=SET-COVERS";
const policyText = "Getoetste tekst & retourinstructies.";
const policies = [
  ["bezorging", "Bezorging"],
  ["retourneren", "Retourneren"],
  ["privacy", "Privacy"],
  ["betalen", "Betalen"],
  ["voorwaarden", "Verkoopvoorwaarden"],
];
const evidence = {
  store: {
    storeName: "Testwinkel",
    sellerName: "Voorbeeld B.V.",
    kvk: "12345678",
    vatId: "NL123456789B01",
    address: "Voorbeeldweg 1\n1234 AB Stad",
    email: "voorbeeld@example.test",
    phone: "+31123456789",
    policies: {
      delivery: policyText,
      returns: policyText,
      privacy: policyText,
      payment: policyText,
      terms: policyText,
    },
  },
  products: [
    {
      sku: "SET-COVERS",
      slug: "test-collectie",
      title: "Hoezen voor & na",
      price: "123.45",
      fill: "covers-only",
      imagePath,
      imageSha256: createHash("sha256").update(photo).digest("hex"),
    },
  ],
};
const availability = "<p>Online bestellen is nog niet beschikbaar.</p>";
const fill = "Alleen hoezen, zonder schuim";
const displayedPrice = "€&nbsp;123,45<span>incl. btw</span>";
const links = policies
  .map(([id]) => `<a href="/nl/winkelinformatie#${id}">${id}</a>`)
  .join("");
const layout = (content, extraHead = "") =>
  `<!doctype html><html lang="nl" dir="ltr"><head><title>Testwinkel</title>${extraHead}</head><body>
    <nav aria-label="Hoofdnavigatie"><a href="/nl/collection">Collectie</a><a href="/nl/winkelinformatie">Winkelinformatie</a></nav>
    <main>${content}</main><footer>${links}</footer></body></html>`;
const collection = layout(`
  <h1>De collectie</h1>${availability}
  <article><img src="${imagePath}" alt="Hoezen"><h2><a href="${productPath}">Hoezen voor &amp; na</a></h2>
  <p>${displayedPrice}</p><p>${fill}</p><p><bdi>SET-COVERS</bdi></p></article>`);
const detail = layout(
  `
  <article data-selected-sku="SET-COVERS"><img src="${imagePath}" alt="Hoezen"><h1>Hoezen voor &amp; na</h1>
  <p data-price="123.45">${displayedPrice}</p><a href="${productPath}" aria-current="true">${fill} · SET-COVERS</a>
  ${availability}</article>`,
  `<link rel="canonical" href="${origin}/nl/collection/test-collectie">`,
);
const information = layout(`
  <h1>Winkelinformatie</h1>
  <address><p>Voorbeeld B.V.</p><p>Voorbeeldweg 1
  1234 AB Stad</p><p><a href="mailto:voorbeeld@example.test">voorbeeld@example.test</a></p>
  <p><a href="tel:+31123456789">+31123456789</a></p></address><dl><dt>KvK</dt><dd>12345678</dd><dt>VAT</dt><dd>NL123456789B01</dd></dl>
  ${policies.map(([id, title]) => `<section id="${id}"><h2>${title}</h2><p>Getoetste tekst &amp; retourinstructies.</p></section>`).join("")}`);

const routes = new Map([
  ["/?variant=discarded", [307, { location: `${origin}/nl` }, null]],
  ["/nl", [307, { location: "/nl/collection" }, null]],
  [
    "/robots.txt",
    [200, { "content-type": "text/plain" }, "User-Agent: *\nAllow: /\n"],
  ],
  [
    "/nl/collection",
    [200, { "content-type": "text/html; charset=utf-8" }, collection],
  ],
  [productPath, [200, { "content-type": "text/html; charset=utf-8" }, detail]],
  [
    "/nl/winkelinformatie",
    [200, { "content-type": "text/html; charset=utf-8" }, information],
  ],
  [imagePath, [200, { "content-type": "image/png" }, photo]],
]);
const headers = {
  "cache-control": "private, no-store",
  "x-robots-tag": "noindex, nofollow",
  "x-piwas-preview-revision": revision,
  "x-content-type-options": "nosniff",
  "referrer-policy": "no-referrer",
};
const csp =
  "default-src 'self'; script-src 'self' 'nonce-A1b2C3d4E5f6G7h8I9j0K1l2' 'strict-dynamic'; style-src 'self' 'nonce-A1b2C3d4E5f6G7h8I9j0K1l2'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'";

function fixture(path, override) {
  const base = routes.get(path) ?? [404, {}, "Niet gevonden"];
  const [status, extra, body] = override?.(path, base) ?? base;
  const htmlHeaders = base[1]["content-type"]?.startsWith("text/html")
    ? { "content-security-policy": csp }
    : {};
  return { status, headers: { ...headers, ...htmlHeaders, ...extra }, body };
}

function runMock(override, observed = []) {
  return runStoreReviewSmoke({
    origin,
    connectionOrigin,
    revision,
    evidence,
    request: async (url, options) => {
      assert.equal(url.origin, connectionOrigin);
      assert.equal(options.redirect, "manual");
      assert.match(options.method, /^(GET|HEAD)$/);
      assert.equal(new Headers(options.headers).has("authorization"), false);
      const path = url.pathname + url.search;
      observed.push([options.method, path]);
      const { status, headers, body } = fixture(path, override);
      return new Response(options.method === "HEAD" ? null : body, {
        status,
        headers,
      });
    },
  });
}

test("readback checks anonymous routes, exact publication and redacted receipt", async () => {
  const observed = [];
  assert.deepEqual(await runMock(undefined, observed), {
    origin,
    revision,
    transport: "private-loopback",
    approvedProducts: 1,
    collectionPages: 1,
  });
  for (const path of [
    ...routes.keys(),
    "/nl/admin",
    "/api/catalogue/nl",
    "/nl/catalogue.xml",
    "/nl/room-planner",
    "/en",
    "/sitemap.xml",
  ]) {
    assert.ok(
      observed.some(
        ([method, requestPath]) => method === "GET" && requestPath === path,
      ),
    );
    assert.ok(
      observed.some(
        ([method, requestPath]) => method === "HEAD" && requestPath === path,
      ),
    );
  }
});

test("readback fails closed on drift, incomplete output or access change", async (t) => {
  const cases = [
    [
      "foreign redirect",
      "/?variant=discarded",
      [307, { location: "https://wrong.example.test/nl" }, null],
      /wrong canonical redirect/,
    ],
    [
      "stale revision",
      "/nl",
      [
        307,
        { location: "/nl/collection", "x-piwas-preview-revision": "stale" },
        null,
      ],
      /wrong served revision/,
    ],
    [
      "robots disallows discovery of noindex",
      "/robots.txt",
      [200, { "content-type": "text/plain" }, "User-Agent: *\nDisallow: /\n"],
      /must permit fetching noindex/,
    ],
    [
      "robots advertises a sitemap",
      "/robots.txt",
      [
        200,
        { "content-type": "text/plain" },
        `User-Agent: *\nAllow: /\nSitemap: ${origin}/sitemap.xml\n`,
      ],
      /without advertising a sitemap/,
    ],
    [
      "cacheable HTML",
      "/nl/collection",
      [200, { "cache-control": "public, no-store" }, collection],
      /cache or indexing directives/,
    ],
    [
      "missing review CSP",
      "/nl/collection",
      [
        200,
        { "content-type": "text/html", "content-security-policy": "" },
        collection,
      ],
      /unexpected content security policy/,
    ],
    [
      "relaxed review CSP",
      "/nl/collection",
      [
        200,
        {
          "content-type": "text/html",
          "content-security-policy": csp.replace(
            "frame-ancestors 'none'",
            "frame-ancestors *",
          ),
        },
        collection,
      ],
      /unexpected content security policy/,
    ],
    [
      "empty collection",
      "/nl/collection",
      [
        200,
        { "content-type": "text/html" },
        layout(`<h1>De collectie</h1>${availability}`),
      ],
      /must contain approved products/,
    ],
    [
      "unexpected SKU",
      "/nl/collection",
      [
        200,
        { "content-type": "text/html" },
        collection.replace(productPath, "/nl/collection/unexpected"),
      ],
      /Unexpected or duplicate/,
    ],
    [
      "visible price drift",
      productPath,
      [
        200,
        { "content-type": "text/html" },
        detail.replace("123,45", "124,45"),
      ],
      /gross product price/,
    ],
    [
      "sale control",
      productPath,
      [
        200,
        { "content-type": "text/html" },
        detail.replace(
          "</article>",
          "<button>Af reken</button><a href='/nl/checkout'>Kopen</a></article>",
        ),
      ],
      /omit purchase controls/,
    ],
    [
      "image digest drift",
      imagePath,
      [
        200,
        { "content-type": "image/png" },
        Buffer.concat([photo, Buffer.from([1])]),
      ],
      /differs from approved image/,
    ],
    [
      "policy drift",
      "/nl/winkelinformatie",
      [
        200,
        { "content-type": "text/html" },
        information.replace("retourinstructies.", "andere tekst."),
      ],
      /delivery policy/,
    ],
    [
      "extra policy paragraph",
      "/nl/winkelinformatie",
      [
        200,
        { "content-type": "text/html" },
        information.replace(
          "</section>",
          "<p>Onbeoordeelde voorwaarde.</p></section>",
        ),
      ],
      /delivery policy/,
    ],
    [
      "duplicate policy section",
      "/nl/winkelinformatie",
      [
        200,
        { "content-type": "text/html" },
        information.replace(
          "</main>",
          '<section id="retourneren"><h2>Retourneren</h2><p>Onbeoordeelde voorwaarde.</p></section></main>',
        ),
      ],
      /returns policy/,
    ],
    [
      "open admin shell",
      "/nl/admin",
      [200, {}, null],
      /Restricted route: unexpected HTTP status/,
    ],
  ];
  for (const [label, path, replacement, message] of cases) {
    await t.test(label, async () => {
      await assert.rejects(
        runMock((actual) => (actual === path ? replacement : undefined)),
        message,
      );
    });
  }
});

test("runner uses only a selected loopback endpoint before cutover", async (t) => {
  const folder = await mkdtemp(join(tmpdir(), "store-review-smoke-"));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const evidencePath = join(folder, "private-input.json");
  await writeFile(evidencePath, JSON.stringify(evidence), { mode: 0o600 });
  const server = createServer((request, response) => {
    const { status, headers, body } = fixture(request.url);
    response.writeHead(status, headers);
    response.end(request.method === "HEAD" ? undefined : body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise((resolve) => server.close(resolve)));
  const args = [
    executable,
    "--origin",
    origin,
    "--revision",
    revision,
    "--evidence-file",
    evidencePath,
    "--candidate-port",
    String(server.address().port),
  ];
  const { stdout, stderr } = await exec(process.execPath, args);
  assert.equal(stderr, "");
  const { observedAt, ...receipt } = JSON.parse(stdout);
  assert.equal(new Date(observedAt).toISOString(), observedAt);
  assert.deepEqual(receipt, {
    origin,
    revision,
    transport: "private-loopback",
    approvedProducts: 1,
    collectionPages: 1,
    evidenceSha256: createHash("sha256")
      .update(JSON.stringify(evidence))
      .digest("hex"),
  });
  assert.doesNotMatch(
    stdout,
    /Testwinkel|Voorbeeldweg|SET-COVERS|private-input/,
  );

  await chmod(evidencePath, 0o644);
  await assert.rejects(exec(process.execPath, args), (error) => {
    assert.match(error.stderr, /private, regular and bounded/);
    assert.doesNotMatch(error.stderr, /private-input|Voorbeeldweg|SET-COVERS/);
    return true;
  });
});

test("evidence rejects mismatched photo identity and unsupported input fields", async (t) => {
  const folder = await mkdtemp(join(tmpdir(), "store-review-evidence-"));
  t.after(() => rm(folder, { recursive: true, force: true }));
  const path = join(folder, "evidence.json");
  const invalid = [
    { ...evidence, token: "unneeded-credential" },
    { ...evidence, products: [] },
    { ...evidence, products: [evidence.products[0], evidence.products[0]] },
    {
      ...evidence,
      products: [
        {
          ...evidence.products[0],
          imagePath: "/catalogue-images/nl/OTHER/4/320",
        },
      ],
    },
    { ...evidence, products: [{ ...evidence.products[0], price: "123.456" }] },
  ];
  for (const input of invalid) {
    await writeFile(path, JSON.stringify(input), { mode: 0o600 });
    await assert.rejects(
      loadEvidence(path),
      /exact seller, policies and distinct approved products/,
    );
  }
});
