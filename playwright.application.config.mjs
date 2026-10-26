import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "tests/application",
  testMatch: "browser.spec.mjs",
  fullyParallel: false,
  workers: 2,
  retries: 0,
  forbidOnly: true,
  use: {
    // Match candidate capture: native browser defaults, with an explicit 1x DPR.
    deviceScaleFactor: 1,
    baseURL: process.env.WEB_BASE_URL,
    httpCredentials: {
      username: "preview",
      password: process.env.PREVIEW_TOKEN ?? "",
    },
    trace: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "firefox", use: { browserName: "firefox" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
  snapshotPathTemplate: "{testDir}/baselines/{projectName}/{arg}{ext}",
});
