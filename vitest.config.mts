import { defineConfig } from "vitest/config";
export default defineConfig({
  test: {
    environment: "jsdom",
    include: [
      "apps/web/tests/**/*.test.tsx",
      "packages/design/tests/**/*.test.tsx",
    ],
    setupFiles: ["./tests/application/setup.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "lcov"],
      reportsDirectory: "coverage/typescript",
      include: ["apps/web/src/**/*.{ts,tsx}", "packages/design/src/**/*.tsx"],
      exclude: [],
      thresholds: { lines: 85, branches: 80, functions: 85, statements: 85 },
    },
  },
});
