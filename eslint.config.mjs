import js from "@eslint/js";
import globals from "globals";
export default [
  {
    ignores: [
      "design/**",
      ".local/**",
      "node_modules/**",
      "coverage/**",
      "test-results/**",
      "playwright-report/**",
      "**/.next/**",
      "target/**",
      "packages/contracts/generated.ts",
      "packages/contracts/models.gen.ts",
      "apps/web/next-env.d.ts",
    ],
  },
  {
    ...js.configs.recommended,
    files: ["**/*.mjs"],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
];
