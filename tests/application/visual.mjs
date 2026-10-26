import { spawnSync } from "node:child_process";
const approval = spawnSync("python3", ["scripts/check_visual_baselines.py"], {
  stdio: "inherit",
});
if (approval.error) throw approval.error;
if (approval.status !== 0) {
  const capture = spawnSync(
    process.execPath,
    ["tests/application/capture.mjs"],
    { env: process.env, stdio: "inherit" },
  );
  if (capture.error) throw capture.error;
  throw new Error(
    "Manual visual-baseline approval required. Candidates captured; visual gate remains failed.",
  );
}
const result = spawnSync(
  "npx",
  [
    "playwright",
    "test",
    "--config",
    "playwright.application.config.mjs",
    "--grep",
    "@visual",
    "--update-snapshots=none",
  ],
  { env: process.env, stdio: "inherit" },
);
if (result.error) throw result.error;
if (result.status !== 0)
  throw new Error("Approved visual baseline differs from served application");
