import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { resolve } from "node:path";
import { availablePort } from "./ports.mjs";
const apiPort = await availablePort();
const webPort = await availablePort();
const token = randomBytes(32).toString("hex");
const environment = {
  ...process.env,
  APP_ENV: "test",
  PREVIEW_TOKEN: token,
  API_BIND: `127.0.0.1:${apiPort}`,
  API_URL: `http://127.0.0.1:${apiPort}`,
  CANONICAL_ORIGIN: `http://127.0.0.1:${webPort}`,
  WEB_BASE_URL: `http://127.0.0.1:${webPort}`,
};
const children = [];
const execute = (command, args) =>
  new Promise((resolve, reject) => {
    const child = spawn(command, args, { env: environment, stdio: "inherit" });
    children.push(child);
    child.on("error", reject);
    child.on("exit", (code) =>
      code === 0 ? resolve() : reject(new Error(`${command} failed (${code})`)),
    );
  });
async function ready(url, authorization) {
  for (let count = 0; count < 120; count++) {
    try {
      const response = await fetch(url, {
        headers: authorization ? { Authorization: authorization } : {},
        signal: AbortSignal.timeout(1000),
      });
      if (response.ok) return;
    } catch {
      /* The owned process may still be starting. */
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("Owned application did not become ready");
}
try {
  await execute("cargo", [
    "test",
    "--locked",
    "-p",
    "commerce-infrastructure",
    "--test",
    "seed_preview",
    "--",
    "--exact",
    "seed_owned_browser_database",
  ]);
  const api = spawn(
    resolve(process.env.CARGO_TARGET_DIR ?? "target", "release/commerce-api"),
    [],
    {
      env: environment,
      stdio: "inherit",
    },
  );
  children.push(api);
  api.on("error", (error) => {
    throw error;
  });
  await ready(`${environment.API_URL}/health/ready`);
  const web = spawn(
    process.execPath,
    [
      "node_modules/next/dist/bin/next",
      "start",
      "apps/web",
      "--hostname",
      "127.0.0.1",
      "--port",
      String(webPort),
    ],
    { env: environment, stdio: "inherit" },
  );
  children.push(web);
  web.on("error", (error) => {
    throw error;
  });
  await ready(
    `${environment.WEB_BASE_URL}/nl`,
    `Basic ${Buffer.from(`preview:${token}`).toString("base64")}`,
  );
  const commands = {
    capture: ["node", ["tests/application/capture.mjs"]],
    e2e: [
      "npx",
      [
        "playwright",
        "test",
        "--config",
        "playwright.application.config.mjs",
        "--grep",
        "@journey",
      ],
    ],
    accessibility: [
      "npx",
      [
        "playwright",
        "test",
        "--config",
        "playwright.application.config.mjs",
        "--grep",
        "@a11y",
      ],
    ],
    visual: ["node", ["tests/application/visual.mjs"]],
    security: [
      "npx",
      [
        "playwright",
        "test",
        "--config",
        "playwright.application.config.mjs",
        "--grep",
        "@security",
      ],
    ],
    discovery: ["node", ["tests/application/discovery.mjs"]],
    performance: ["node", ["tests/application/performance.mjs"]],
  };
  const selected = process.argv.slice(2);
  if (!selected.length) throw new Error("Select real application checks");
  for (const name of selected) {
    const entry = commands[name];
    if (!entry) throw new Error(`Unknown check ${name}`);
    await execute(...entry);
  }
} finally {
  for (const child of children.reverse())
    if (child.exitCode === null) child.kill("SIGTERM");
}
