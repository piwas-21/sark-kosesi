import assert from "node:assert/strict";
import { once } from "node:events";
import { createServer } from "node:http";
import {
  approved,
  dimensions,
  page,
} from "../../../apps/web/tests/published-fixture.ts";

export const image = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jQzgAAAAASUVORK5CYII=",
  "base64",
);
export const covers = {
  ...approved,
  images: approved.images.map((item) => ({ ...item, width: 1, height: 1 })),
};
export const foam = {
  ...covers,
  sku: "SET-FOAM",
  fill: "with-foam",
  minor_units: 23456,
  price_decimal: "234.56",
  copy: { ...covers.copy, title: "Uitvoering met schuim voor tests" },
  components: [
    ...covers.components,
    {
      code: "SEAT-FOAM",
      quantity: 2,
      dimensions,
      material: { kind: "foam", density_g_per_m3: 25000 },
    },
  ],
  images: [
    { ...covers.images[0], url: "/api/published/nl/images/SET-FOAM/1/320" },
  ],
};

// This loopback-only double supplies the reviewed projection, not a catalogue writer.
export async function startUpstream(token) {
  let state = "ready";
  const server = createServer((request, response) => {
    if (request.method !== "GET") return response.writeHead(405).end();
    if (request.url === "/health/ready") return response.writeHead(204).end();
    if (request.headers.authorization !== `Bearer ${token}`)
      return response.writeHead(401).end();
    if (state === "unavailable") return response.writeHead(503).end();
    if (request.url === "/api/published/nl") {
      const result = state === "empty" ? page([]) : page([], "ARCHIVED-A");
      return response
        .writeHead(200, { "content-type": "application/json" })
        .end(JSON.stringify(result));
    }
    if (
      state === "ready" &&
      request.url === "/api/published/nl?after=ARCHIVED-A"
    )
      return response
        .writeHead(200, { "content-type": "application/json" })
        .end(JSON.stringify(page([covers, foam])));
    if (
      state === "ready" &&
      [covers.images[0].url, foam.images[0].url].includes(request.url)
    )
      return response
        .writeHead(200, { "content-type": "image/png" })
        .end(image);
    return response.writeHead(404).end();
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return {
    origin: `http://127.0.0.1:${address.port}`,
    setState(next) {
      state = next;
    },
    close() {
      return new Promise((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
        server.closeAllConnections();
      });
    },
  };
}
