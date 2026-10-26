import { describe, it, expect } from "vitest";
import { decodeCatalogue } from "../src/lib/catalogue";
import { configuration } from "../src/lib/config";
import {
  deviceLocale,
  direction,
  isLocale,
  languageTag,
  resolveLocale,
  switchLocale,
} from "../src/lib/locale";
import { isTheme, resolveTheme, themeBootstrap } from "../src/lib/theme";
const item = {
  sku: "DEMO-COURTYARD-COVERS",
  slug: "courtyard-demo",
  title: "Demonstration seating",
  description: "Not for sale",
  care: "Pending",
  minor_units: 12345,
  currency: "EUR",
  fill: "covers-only",
  fixture: true,
};
const valid = { items: [item], locale: "nl", production_enabled: false };
describe("untrusted catalogue response", () => {
  it("accepts independent fixture values", () => {
    expect(decodeCatalogue(valid)).toEqual(valid);
    expect(decodeCatalogue({ ...valid, items: [] })).toEqual({
      ...valid,
      items: [],
    });
  });
  it("rejects malformed and unsafe data", () => {
    for (const input of [
      null,
      [],
      {},
      true,
      { ...valid, items: {} },
      { ...valid, items: Array(51).fill(item) },
      { ...valid, production_enabled: true },
      { ...valid, locale: 12 },
    ])
      expect(() => decodeCatalogue(input)).toThrow();
    for (const patch of [
      { sku: "REAL-1" },
      { slug: "../secret" },
      { title: 1 },
      { title: "a".repeat(161) },
      { description: 1 },
      { care: 1 },
      { minor_units: -1 },
      { minor_units: 1.2 },
      { minor_units: Number.MAX_SAFE_INTEGER + 1 },
      { currency: "USD" },
      { fill: "mystery" },
      { fill: 3 },
      { fixture: false },
    ])
      expect(() =>
        decodeCatalogue({ ...valid, items: [{ ...item, ...patch }] }),
      ).toThrow();
    expect(() => decodeCatalogue({ ...valid, items: [null] })).toThrow();
  });
});
describe("locale policy", () => {
  it("resolves language mappings without claiming Sorani", () => {
    for (const tag of ["kmr", "kmr-TR", "ku", "ku-Latn", "ku-Latn-TR"])
      expect(deviceLocale(tag)).toBe("ku");
    for (const tag of ["ckb", "ku-Arab", "ku-IQ", "zz"])
      expect(deviceLocale(tag)).toBeUndefined();
    expect(deviceLocale("DE_de")).toBe("de");
    expect(languageTag("ku")).toBe("ku-Latn");
    expect(languageTag("nl")).toBe("nl");
    expect(direction("ar")).toBe("rtl");
    expect(direction("tr")).toBe("ltr");
    expect(isLocale("kmr")).toBe(false);
  });
  it("honours stored choice and weighted device preferences", () => {
    expect(resolveLocale("fr", "de;q=1")).toBe("fr");
    expect(resolveLocale("bad", "de;q=0.2, ar;q=0.8")).toBe("ar");
    expect(resolveLocale(undefined, "de;q=0,en;q=0.5")).toBe("en");
    expect(resolveLocale(undefined, "xx, fr;q=0.3")).toBe("fr");
    expect(resolveLocale(undefined, "de;q=NaN,ar;q=2,en;q=-1")).toBe("nl");
    expect(resolveLocale(undefined, "")).toBe("nl");
    expect(
      switchLocale("/nl/products/demo", "?variant=covers&token=required", "ar"),
    ).toBe("/ar/products/demo?variant=covers&token=required");
    expect(switchLocale("/products/demo", "", "de")).toBe("/de/products/demo");
  });
});
it("validates every environment input and fails closed", () => {
  const values = {
    APP_ENV: "test",
    PREVIEW_TOKEN: "a".repeat(32),
    API_URL: "http://127.0.0.1:4000",
    CANONICAL_ORIGIN: "http://127.0.0.1:3000",
  };
  expect(configuration(values).api).toBe("http://127.0.0.1:4000");
  for (const patch of [
    { APP_ENV: "production" },
    { PREVIEW_TOKEN: "" },
    { API_URL: "https://127.0.0.1" },
    { API_URL: "http://evil.test" },
    { API_URL: "http://user:pass@127.0.0.1" },
    { API_URL: "http://127.0.0.1/path" },
    { API_URL: "http://127.0.0.1?query" },
    { CANONICAL_ORIGIN: "http://127.0.0.1#hash" },
    { API_URL: "" },
  ])
    expect(() => configuration({ ...values, ...patch })).toThrow();
});
it("resolves theme with explicit preference and safe first-paint bootstrap", () => {
  expect(isTheme("system")).toBe(true);
  expect(isTheme(null)).toBe(false);
  expect(resolveTheme("dark", false)).toBe("dark");
  expect(resolveTheme("light", true)).toBe("light");
  expect(resolveTheme("system", true)).toBe("dark");
  expect(resolveTheme("unknown", false)).toBe("light");
  localStorage.setItem("theme", "dark");
  window.eval(themeBootstrap);
  expect(document.documentElement.dataset.theme).toBe("dark");
});
