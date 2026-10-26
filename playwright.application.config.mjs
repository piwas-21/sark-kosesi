import { defineConfig, devices } from "@playwright/test";
export default defineConfig({
  testDir: "tests/application",
  testMatch: "browser.spec.mjs",
  fullyParallel: false,
  workers: 2,
  retries: 0,
  forbidOnly: true,
  use: {
    baseURL: process.env.WEB_BASE_URL,
    httpCredentials: {
      username: "preview",
      password: process.env.PREVIEW_TOKEN ?? "",
    },
    trace: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],
  snapshotPathTemplate: "{testDir}/baselines/{projectName}/{arg}{ext}",
});
