import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { visualContext, themeReady } from "./visual-context.mjs";
const locales = ["nl", "de", "fr", "en", "ar", "tr", "ku"];
async function assertGalleryTabOrder(page, browserName) {
  const forward =
    browserName === "webkit" && process.platform === "darwin"
      ? "Alt+Tab"
      : "Tab";
  const reverse =
    browserName === "webkit" && process.platform === "darwin"
      ? "Alt+Shift+Tab"
      : "Shift+Tab";
  // Reset only the starting point; every destination is reached by actual keys.
  await page.evaluate(() => {
    document.body.tabIndex = -1;
    document.body.focus();
    document.body.removeAttribute("tabindex");
  });
  const order = [
    page.getByRole("link", { name: "Skip to content" }),
    page.getByRole("link", { name: "dicle." }),
    page.getByRole("link", { name: "Collection", exact: true }),
    page.getByRole("link", { name: "Product preview", exact: true }),
    page.getByRole("link", { name: "Components", exact: true }),
    page.getByRole("link", { name: "Admin", exact: true }),
    page.getByLabel("Language"),
    page.getByLabel("Theme"),
    page.getByRole("button", { name: "Preview action" }),
    page.getByLabel("Room width (cm)"),
  ];
  for (const destination of order) {
    await page.keyboard.press(forward);
    await expect(destination).toBeFocused({ timeout: 500 });
  }
  await page.keyboard.press(reverse);
  await expect(
    page.getByRole("button", { name: "Preview action" }),
  ).toBeFocused();
}

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
      browserName,
    }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.goto(`/${locale}/gallery`);
      await page.getByLabel("Theme").selectOption(theme);
      await assertGalleryTabOrder(page, browserName);
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
  test.describe(`${viewport.width}px visual context`, () => {
    test.use(visualContext(viewport));
    for (const surface of [
      { name: "courtyard", path: "", locales: ["nl", "ar"] },
      { name: "gallery", path: "/gallery", locales },
      { name: "collection", path: "/collection", locales: ["nl", "ar"] },
      {
        name: "covers",
        path: "/collection/isolated-room?variant=SET-COVERS",
        locales: ["nl", "ar"],
      },
      {
        name: "filled",
        path: "/collection/isolated-room?variant=SET-FILLED",
        locales: ["nl", "ar"],
      },
      { name: "empty", path: "/collection", locales: ["en", "ku"] },
      {
        name: "missing",
        path: "/collection/missing?variant=MISSING",
        locales: ["nl", "ar"],
      },
    ]) {
      for (const locale of surface.locales)
        for (const theme of ["light", "dark"]) {
          test(`@visual ${surface.name} ${locale} ${theme} ${viewport.width}`, async ({
            page,
          }) => {
            // The declared fixture theme is also the device preference from first paint.
            await page.emulateMedia({ colorScheme: theme });
            await page.goto(`/${locale}${surface.path}`);
            await page.getByLabel("Theme").selectOption(theme);
            if (surface.name === "gallery")
              await page.getByLabel("Room width (cm)").focus();
            await themeReady(page, theme);
            await expect(page).toHaveScreenshot(
              `${surface.name}-${locale}-${theme}-${viewport.width}.png`,
              { fullPage: true, animations: "disabled" },
            );
          });
        }
    }
  });
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

test("@a11y traversal detects an input removed from keyboard order", async ({
  page,
  browserName,
}) => {
  await page.goto("/nl/gallery");
  await assertGalleryTabOrder(page, browserName);
  await page.getByLabel("Room width (cm)").evaluate((input) => {
    input.tabIndex = -1;
  });
  await expect(assertGalleryTabOrder(page, browserName)).rejects.toThrow(
    /Room width/,
  );
});

test("@journey exact configurations and language changes preserve the selected SKU", async ({
  page,
}) => {
  await page.goto("/nl/collection/isolated-room?variant=SET-COVERS");
  await expect(page.locator("[data-price]")).toHaveAttribute(
    "data-price",
    "98.76",
  );
  await expect(page.locator("[data-availability]")).toHaveAttribute(
    "data-availability",
    "in_stock",
  );
  await page.getByRole("link", { name: /with-foam.*SET-FILLED/ }).click();
  await expect(page).toHaveURL(/variant=SET-FILLED$/);
  await expect(page.locator("[data-price]")).toHaveAttribute(
    "data-price",
    "123.45",
  );
  await expect(page.locator("[data-availability]")).toHaveAttribute(
    "data-availability",
    "out_of_stock",
  );
  await expect(page.getByText("35000 g/m³")).toBeVisible();
  await page.getByLabel("Language").selectOption("ar");
  await expect(page).toHaveURL(
    /\/ar\/collection\/isolated-ar-room\?variant=SET-FILLED$/,
  );
  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator('a[aria-current="true"]')).toHaveAttribute(
    "href",
    /variant=SET-FILLED$/,
  );
  await page.getByLabel("Language").selectOption("ku");
  await expect(page.locator("html")).toHaveAttribute("lang", "ku-Latn");
  await expect(page.locator("[data-price]")).toHaveCount(0);
});
for (const locale of ["nl", "ar"])
  for (const theme of ["light", "dark"])
    test(`@a11y approved configuration ${locale} ${theme}`, async ({
      page,
      browserName,
    }) => {
      await page.setViewportSize({ width: 390, height: 844 });
      await page.goto(`/${locale}/collection/isolated-room?variant=SET-COVERS`);
      await page.getByLabel("Theme").selectOption(theme);
      expect(
        (
          await new AxeBuilder({ page })
            .withTags(["wcag2a", "wcag2aa", "wcag21aa", "wcag22aa"])
            .analyze()
        ).violations,
      ).toEqual([]);
      await page.evaluate(() => {
        document.body.tabIndex = -1;
        document.body.focus();
        document.body.removeAttribute("tabindex");
      });
      const forward =
        browserName === "webkit" && process.platform === "darwin"
          ? "Alt+Tab"
          : "Tab";
      await page.keyboard.press(forward);
      await expect(
        page.getByRole("link", { name: "Skip to content" }),
      ).toBeFocused();
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
    });

test("@journey default family selection survives locale-specific family slugs", async ({
  page,
}) => {
  await page.goto("/nl/collection/isolated-room");
  await expect(page.locator("article[data-selected-sku]")).toHaveAttribute(
    "data-selected-sku",
    "SET-COVERS",
  );
  await page.getByLabel("Language").selectOption("ar");
  await expect(page).toHaveURL(
    /\/ar\/collection\/isolated-ar-room\?variant=SET-COVERS$/,
  );
  await expect(page.locator("[data-price]")).toHaveAttribute(
    "data-price",
    "98.76",
  );
  await expect(page.locator("article[data-selected-sku]")).toHaveAttribute(
    "data-selected-sku",
    "SET-COVERS",
  );
});
