import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
const locales = ["nl", "de", "fr", "en", "ar", "tr", "ku"];
test("@journey seeded catalogue is readable before hydration", async ({
  page,
}) => {
  await page.goto("/nl");
  await expect(
    page.getByRole("heading", { name: "Blijf nog even." }),
  ).toBeVisible();
  await page.getByRole("link", { name: "Demonstration seating" }).click();
  await expect(page.getByText("DEMO-COURTYARD-COVERS")).toBeVisible();
  await expect(
    page.getByText("Checkout is disabled.", { exact: false }),
  ).toBeVisible();
  await page.getByLabel("Language").selectOption("ar");
  await expect(page).toHaveURL(/\/ar\/products\/courtyard-demo/);
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "ar");
});
test("@journey explicit locale and first-paint theme survive denied storage", async ({
  page,
}) => {
  await page.addInitScript(() => {
    Object.defineProperty(Storage.prototype, "getItem", {
      value() {
        throw new Error("denied");
      },
    });
    Object.defineProperty(Storage.prototype, "setItem", {
      value() {
        throw new Error("denied");
      },
    });
  });
  await page.goto("/fr");
  await page.getByLabel("Theme").selectOption("dark");
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page.getByLabel("Language").selectOption("ku");
  await expect(page.locator("html")).toHaveAttribute("lang", "ku-Latn");
});
for (const locale of locales)
  for (const theme of ["light", "dark"]) {
    test(`@a11y ${locale} ${theme} gallery keyboard and axe`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.goto(`/${locale}/gallery`);
      await page.getByLabel("Theme").selectOption(theme);
      await page.keyboard.press("Tab"); // Keyboard navigation is exercised independently of axe.
      await page.getByLabel("Room width (cm)").focus();
      await expect(page.getByLabel("Room width (cm)")).toBeFocused();
      const result = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
        .analyze();
      expect(result.violations).toEqual([]);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
    });
  }
for (const viewport of [
  { width: 1280, height: 900 },
  { width: 390, height: 844 },
]) {
  for (const surface of [
    { name: "courtyard", path: "", locales: ["nl", "ar"] },
    { name: "gallery", path: "/gallery", locales },
  ]) {
    for (const locale of surface.locales)
      for (const theme of ["light", "dark"]) {
        test(`@visual ${surface.name} ${locale} ${theme} ${viewport.width}`, async ({
          page,
        }) => {
          await page.setViewportSize(viewport);
          await page.emulateMedia({ reducedMotion: "reduce" });
          await page.goto(`/${locale}${surface.path}`);
          await page.getByLabel("Theme").selectOption(theme);
          if (surface.name === "gallery")
            await page.getByLabel("Room width (cm)").focus();
          await page.evaluate(() => document.fonts.ready);
          await expect(page).toHaveScreenshot(
            `${surface.name}-${locale}-${theme}-${viewport.width}.png`,
            { fullPage: true, animations: "disabled" },
          );
        });
      }
  }
}
test("@security auth, private cache, CSP and unavailable mutations", async ({
  page,
}) => {
  const response = await fetch(`${process.env.WEB_BASE_URL}/nl`, {
    redirect: "manual",
  });
  expect(response.status).toBe(401);
  expect(response.headers.get("x-robots-tag")).toContain("noindex");
  const result = await page.goto("/nl");
  expect(result?.headers()["cache-control"]).toContain("no-store");
  expect(result?.headers()["content-security-policy"]).toContain(
    "frame-ancestors 'none'",
  );
  const read = await page.request.get("/api/catalogue/nl");
  expect(read.status()).toBe(200);
  expect((await read.json()).production_enabled).toBe(false);
  const mutation = await page.request.post("/api/catalogue/nl", {
    data: { minor_units: 1 },
  });
  expect(mutation.status()).toBe(405);
  for (const path of [
    "/api/orders",
    "/api/admin",
    "/nl/account/orders",
    "/nl/checkout",
  ])
    expect((await page.request.get(path)).status()).toBe(404);
});
