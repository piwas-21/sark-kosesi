import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { readFile } from "node:fs/promises";
import { visualContext, themeReady } from "./visual-context.mjs";
import { fillPlannerPreview } from "./planner-fixture.mjs";
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
test("@journey visual readiness rejects stale descendant colours", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await page.goto("/nl/collection/missing?variant=MISSING");
  await page.getByLabel("Theme").selectOption("dark");
  await themeReady(page, "dark");
  for (const [selector, property] of [
    ["main h1", "color"],
    ['header nav[aria-label="Main"] a:last-child', "color"],
    ["header .brand svg path", "stroke"],
  ]) {
    await page.locator(selector).evaluate((element, name) => {
      element.style.setProperty(name, "rgb(255, 0, 0)", "important");
    }, property);
    await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
    await expect(page.locator("body")).toHaveCSS("color", "rgb(232, 237, 207)");
    await expect(page.locator(selector)).toHaveCSS(property, "rgb(255, 0, 0)");
    await expect(themeReady(page, "dark", 250)).rejects.toThrow(/Timeout/);
    await page.locator(selector).evaluate((element, name) => {
      element.style.removeProperty(name);
    }, property);
    await themeReady(page, "dark");
  }
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
      { name: "planner", path: "/room-planner", locales: ["nl", "ar"] },
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
            if (surface.name === "planner") await fillPlannerPreview(page);
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

test("@journey room planner downloads and confirms a local plan file", async ({
  page,
}) => {
  const writes = [];
  page.on("request", (request) => {
    if (!["GET", "HEAD"].includes(request.method()))
      writes.push(request.method());
  });
  await page.goto("/en/room-planner");
  await fillPlannerPreview(page);
  const files = page.getByRole("region", { name: "Plan file" });
  const result = page
    .getByRole("complementary", { name: "Floor plan" })
    .getByRole("status");
  await expect(result).toHaveAttribute("data-state", "fit");
  const downloadPromise = page.waitForEvent("download");
  await files.getByRole("button", { name: "Download a copy" }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe("room-plan.json");
  const document = JSON.parse(await readFile(await download.path(), "utf8"));
  expect(document.format).toBe("piwas-room-plan");
  expect(document.room.width).toBe("430");
  document.unit = "mm";
  document.room.width = "5000";
  document.room.depth = "3300";
  const input = files.locator('input[type="file"]');
  const payload = {
    name: "room-plan.json",
    mimeType: "application/json",
    buffer: Buffer.from(JSON.stringify(document)),
  };
  await input.setInputFiles(payload);
  await expect(files.getByRole("status")).toContainText(
    "This plan will replace the current measurements and unit.",
  );
  await expect(page.locator("#plan-room-width")).toHaveValue("430");
  await expect(result).toHaveAttribute("data-state", "fit");
  await files.getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator("#plan-room-width")).toHaveValue("430");
  await expect(
    files.getByRole("button", { name: "Open a plan file" }),
  ).toBeFocused();
  await input.setInputFiles(payload);
  await files.getByRole("button", { name: "Use this plan" }).click();
  await expect(page.locator("#plan-room-width")).toHaveValue("5000");
  await expect(page.locator("#plan-unit")).toHaveValue("mm");
  await expect(result).not.toHaveAttribute("data-state", "fit");
  await expect(
    files.getByRole("button", { name: "Open a plan file" }),
  ).toBeFocused();
  expect(writes).toEqual([]);
});

test("@journey room planner checks rotation and clearance locally", async ({
  page,
}) => {
  await page.goto("/en/room-planner");
  const room = page.getByRole("region", { name: "Room", exact: true });
  const piece = page.getByRole("group", { name: "Piece 1" });
  const status = page
    .getByRole("complementary", { name: "Floor plan" })
    .getByRole("status");
  const check = page.getByRole("button", { name: "Check the layout" });
  const writes = [];
  page.on("request", (request) => {
    if (!["GET", "HEAD"].includes(request.method()))
      writes.push(request.method());
  });
  await room.getByLabel("Width (cm)").fill("400");
  await room.getByLabel("Depth (cm)").fill("300");
  await piece.getByLabel("Width (cm)").fill("150");
  await piece.getByLabel("Depth (cm)").fill("70");
  await piece.getByLabel("From the left (cm)").fill("280");
  await expect(page.getByLabel("Unit of measurement")).toBeDisabled();
  await check.focus();
  await page.keyboard.press("Enter");
  await expect(status).toContainText("Piece 1 extends beyond the room.");

  await piece.getByLabel("Rotation").selectOption("90");
  await check.click();
  await expect(status).toContainText("Confirm the door clearance");
  const clearance = page.getByLabel("What space needs to stay clear?");
  await clearance.selectOption("none");
  await check.click();
  await expect(status).toContainText("The shapes fit within your measurements");

  await clearance.selectOption("measured");
  const door = page.getByRole("group", { name: "Clearance zone 1" });
  await door.getByLabel("Width (cm)").fill("20");
  await door.getByLabel("Depth (cm)").fill("30");
  await door.getByLabel("From the left (cm)").fill("280");
  await check.click();
  await expect(status).toContainText("Piece 1 blocks Clearance zone 1.");
  await page.getByRole("button", { name: "Add a piece" }).click();
  await expect(
    page.getByRole("group", { name: "Piece 2" }).getByLabel("Width (cm)"),
  ).toBeFocused();
  await expect(page).toHaveURL(/\/en\/room-planner$/);
  expect(writes).toEqual([]);
  await page.reload();
  await expect(room.getByLabel("Width (cm)")).toHaveValue("");
  await expect(page.getByLabel("Unit of measurement")).toBeEnabled();
  await room.getByLabel("Width (cm)").fill("400");
  await page.getByLabel("Language").selectOption("ar");
  await expect(page).toHaveURL(/\/ar\/room-planner$/);
  await expect(page.locator("#plan-room-width")).toHaveValue("");
});

test("@a11y long room planner conflicts remain reachable by page scrolling", async ({
  page,
  browserName,
}) => {
  await page.setViewportSize({ width: 1280, height: 500 });
  await page.goto("/en/room-planner");
  const room = page.getByRole("region", { name: "Room", exact: true });
  await room.getByLabel("Width (cm)").fill("400");
  await room.getByLabel("Depth (cm)").fill("300");
  for (let number = 1; number <= 8; number++) {
    if (number > 1)
      await page.getByRole("button", { name: "Add a piece" }).click();
    const piece = page.getByRole("group", { name: `Piece ${number}` });
    await piece.getByLabel("Width (cm)").fill("100");
    await piece.getByLabel("Depth (cm)").fill("100");
  }
  await page.getByLabel("What space needs to stay clear?").selectOption("none");
  await page.getByRole("button", { name: "Check the layout" }).click();

  const list = page.getByRole("list", { name: "Adjust this layout." });
  const issues = list.getByRole("listitem");
  await expect(issues).toHaveCount(28);
  await expect(issues.last()).toHaveText("Piece 8 overlaps Piece 7.");
  await expect(list).not.toHaveAttribute("tabindex");
  expect(
    await list.evaluate(
      (element) => element.scrollHeight <= element.clientHeight,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "Clear measurements" }).focus();
  await page.keyboard.press(
    browserName === "webkit" && process.platform === "darwin"
      ? "Alt+Tab"
      : "Tab",
  );
  await expect(list).not.toBeFocused();

  // Start at the page top, then use only native keyboard scrolling to reach the end.
  await page.evaluate(() => {
    document.body.tabIndex = -1;
    document.body.focus();
    document.body.removeAttribute("tabindex");
    window.scrollTo(0, 0);
  });
  for (let count = 0; count < 30; count++) {
    const inViewport = await issues.last().evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      return bounds.top >= 0 && bounds.bottom <= window.innerHeight;
    });
    if (inViewport) break;
    await page.keyboard.press("PageDown");
    await page.waitForTimeout(100);
  }
  await expect(issues.last()).toBeInViewport({ ratio: 1 });
  expect(await page.evaluate(() => window.scrollY)).toBeGreaterThan(0);
});

for (const locale of ["nl", "ar"])
  for (const theme of ["light", "dark"]) {
    test(`@a11y room planner ${locale} ${theme} narrow layout`, async ({
      page,
    }) => {
      await page.setViewportSize({ width: 320, height: 700 });
      const response = await page.goto(`/${locale}/room-planner`);
      expect(response?.headers()["x-robots-tag"]).toContain("noindex");
      await page.getByLabel("Theme").selectOption(theme);
      await expect(page.locator("html")).toHaveAttribute(
        "dir",
        locale === "ar" ? "rtl" : "ltr",
      );
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
