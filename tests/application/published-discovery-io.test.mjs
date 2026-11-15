import assert from "node:assert/strict";
import { createServer } from "node:http";
import { once } from "node:events";
import { test } from "node:test";
import "jsdom";
import {
  fetchWithoutReuse,
  runFixtureProcess,
} from "./published-discovery-io.mjs";

test("discovery fetch closes each authenticated fixture connection", async () => {
  const sockets = new Set();
  const headers = [];
  const server = createServer((request, response) => {
    sockets.add(request.socket);
    headers.push(request.headers);
    response.end("fixture");
  });
  server.keepAliveTimeout = 50;
  server.keepAliveTimeoutBuffer = 0;
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const url = `http://127.0.0.1:${server.address().port}/`;
    for (let i = 0; i < 2; i++) {
      const response = await fetchWithoutReuse(url, {
        headers: { Authorization: "Basic synthetic", Connection: "keep-alive" },
      });
      assert.equal(await response.text(), "fixture");
      if (i === 0)
        await runFixtureProcess(
          process.execPath,
          ["-e", "setTimeout(() => {}, 120)"],
          3000,
        );
    }
    assert.equal(sockets.size, 2);
    assert.equal(headers.length, 2);
    for (const header of headers) {
      assert.equal(header.connection, "close");
      assert.equal(header.authorization, "Basic synthetic");
    }
  } finally {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
});

test("bounded fixture child does not block the event loop", async () => {
  let timerFired = false;
  const timer = setTimeout(() => {
    timerFired = true;
  }, 20);
  try {
    await runFixtureProcess(
      process.execPath,
      ["-e", "setTimeout(() => {}, 120)"],
      3000,
    );
    assert.equal(timerFired, true);
  } finally {
    clearTimeout(timer);
  }
});

test("nonzero and timed out fixture children fail without exposing output", async () => {
  await assert.rejects(
    runFixtureProcess("sark-nonexistent-fixture-command", [], 3000),
    /Owned fixture effect failed; details withheld/,
  );
  await assert.rejects(
    runFixtureProcess(
      process.execPath,
      ["-e", "console.error('synthetic-secret');process.exit(1)"],
      3000,
    ),
    (error) =>
      error.message === "Owned fixture effect failed; details withheld" &&
      !error.message.includes("synthetic-secret"),
  );
  const startedAt = Date.now();
  await assert.rejects(
    runFixtureProcess(
      process.execPath,
      ["-e", "setTimeout(() => {}, 10000)"],
      100,
    ),
    /Owned fixture effect failed; details withheld/,
  );
  assert.ok(
    Date.now() - startedAt < 5000,
    "timed out child must stop promptly",
  );
});
