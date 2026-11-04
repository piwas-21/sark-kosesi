import { spawnSync } from "node:child_process";
import { appendFileSync } from "node:fs";
const approval = spawnSync("python3", ["scripts/check_visual_baselines.py"], {
  stdio: "inherit",
});
if (approval.error) throw approval.error;
if (approval.signal)
  throw new Error(`Visual approval check terminated (${approval.signal})`);
if (approval.status !== 0) {
  const capture = spawnSync(
    process.execPath,
    ["tests/application/capture.mjs"],
    { env: process.env, stdio: "inherit" },
  );
  if (capture.error) throw capture.error;
  if (capture.status !== 0 || capture.signal)
    throw new Error(
      `Visual candidate capture failed (${capture.signal ?? capture.status})`,
    );
  if (process.env.GITHUB_OUTPUT)
    appendFileSync(process.env.GITHUB_OUTPUT, "visual_candidates=complete\n");
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
