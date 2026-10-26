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
    ],
  },
  {
    ...js.configs.recommended,
    files: ["**/*.mjs"],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
];
