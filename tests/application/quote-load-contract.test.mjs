import assert from "node:assert/strict";
import test from "node:test";
import {
  BURST_OWNERS,
  STEADY_MS,
  STEADY_OWNERS,
  STEADY_SLEEP_SECONDS,
  quoteEffect,
  scenarios,
} from "./quote-load-contract.mjs";

test("workload identities are distinct and do not exceed the fixed budget", () => {
  const plan = scenarios();
  assert.equal(STEADY_OWNERS, 25);
  assert.equal(BURST_OWNERS, 50);
  assert.equal(STEADY_MS, 600_000);
  assert.ok(STEADY_SLEEP_SECONDS > 10);
  assert.equal(Object.keys(plan).length, 26);
  const names = new Set();
  for (let owner = 0; owner < STEADY_OWNERS; owner += 1) {
    const item = plan[`quote_steady_${owner}`];
    assert.equal(item.vus, 1);
    assert.equal(item.duration, "10m");
    assert.equal(item.env.QUOTE_OWNER, String(owner));
    assert.equal(item.tags.workload, "steady");
    for (let sequence = 0; sequence < 60; sequence += 1) {
      names.add(quoteEffect("steady", owner, sequence));
    }
  }
  assert.equal(plan.quote_burst.vus, 50);
  assert.equal(plan.quote_burst.iterations, 1);
  assert.equal(plan.quote_burst.executor, "per-vu-iterations");
  for (let owner = 0; owner < BURST_OWNERS; owner += 1) {
    names.add(quoteEffect("burst", owner, 0));
  }
  assert.equal(names.size, 1550);
});

test("invalid synthetic positions fail without echoing input", () => {
  for (const args of [
    ["private-marker", 0, 0],
    ["steady", 25, 0],
    ["burst", 50, 0],
    ["burst", 1, 1],
    ["steady", 1.5, 0],
    ["steady", 1, -1],
  ]) {
    assert.throws(
      () => quoteEffect(...args),
      /^Error: invalid synthetic quote position$/,
    );
  }
});
