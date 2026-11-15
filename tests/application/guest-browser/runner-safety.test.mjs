import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const runner = "tests/application/guest-browser/runner.mjs";
function rejected(url, extra = {}) {
  const result = spawnSync(process.execPath, [runner], {
    env: { ...process.env, APP_ENV: "test", DATABASE_URL: url, ...extra },
    encoding: "utf8",
    timeout: 15000,
  });
  assert.notEqual(result.status, 0);
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  assert.match(output, /Invalid disposable PostgreSQL configuration/);
  return output;
}

test("malformed database URL and connect options fail without disclosing input", () => {
  const credential = "synthetic-secret-do-not-print-9e21";
  const endpoint = "private-synthetic-endpoint.invalid";
  const output = rejected(`postgresql://${credential}@${endpoint}:[invalid`);
  assert(!output.includes(credential));
  assert(!output.includes(endpoint));
  const goodShape = `postgresql://p1_test:${"a".repeat(43)}@127.0.0.1:12345/postgres`;
  rejected(`${goodShape}?options=-csark.disposable_marker%3Dforged`);
  rejected(`${goodShape}#fragment`);
  rejected(goodShape, { PGOPTIONS: "-c sark.disposable_marker=forged" });
});
