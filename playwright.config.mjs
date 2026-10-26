import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests/prototype',
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  workers: process.env.CI ? 2 : undefined,
  timeout: 30_000,
  reporter: [['list'], ['html', { open: 'never' }]],
  use: {
    baseURL: 'http://127.0.0.1:48764',
    browserName: 'chromium',
    viewport: { width: 1280, height: 800 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'python3 scripts/serve_preview.py',
    env: { PORT: '48764' },
    url: 'http://127.0.0.1:48764/studio.html',
    reuseExistingServer: false,
    timeout: 15_000,
  },
});
