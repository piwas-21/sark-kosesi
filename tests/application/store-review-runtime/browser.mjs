import assert from "node:assert/strict";
import AxeBuilder from "@axe-core/playwright";
import { chromium, expect } from "@playwright/test";
import { reviewContent } from "../../../apps/web/tests/store-review-fixture.ts";
import { covers, foam } from "./upstream.mjs";

async function assertAccessibility(page) {
  const findings = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
  assert.deepEqual(
    findings.violations.map(({ id, nodes }) => [
      id,
      nodes.map(({ target }) => target),
    ]),
    [],
  );
  assert.ok(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
    "The review content must fit its viewport",
  );
}

async function customerReads(page, origin) {
  const heading = (name) => page.getByRole("heading", { name, exact: true });
  const article = page.getByRole("article");
  const detail = `${origin}/nl/collection/isolated-room`;
  await page.goto(`${origin}/nl`);
  await expect(page).toHaveURL(`${origin}/nl/collection`);
  await expect(page.locator("html")).toHaveAttribute("lang", "nl");
  await expect(heading("De collectie")).toBeVisible();
  await page.getByRole("link", { name: covers.copy.title }).click();
  await expect(page).toHaveURL(`${detail}?variant=SET-COVERS`);
  await expect(article).toHaveAttribute("data-selected-sku", "SET-COVERS");
  await expect(article.locator("[data-price]")).toHaveAttribute(
    "data-price",
    "123.45",
  );
  await expect(article.locator("[data-price]")).toContainText(
    /€\s*123,45.*incl\. btw/,
  );
  await expect(
    article.getByText("Online bestellen is nog niet beschikbaar."),
  ).toBeVisible();
  const variants = page.getByRole("navigation", { name: "Uitvoering" });
  await expect(
    variants.getByRole("link", { name: /Alleen hoezen/ }),
  ).toHaveAttribute("aria-current", "true");
  await variants.getByRole("link", { name: /Met schuimvulling/ }).click();
  await expect(page).toHaveURL(`${detail}?variant=SET-FOAM`);
  await expect(article).toHaveAttribute("data-selected-sku", "SET-FOAM");
  await expect(heading(foam.copy.title)).toBeVisible();
  await expect(article.locator("[data-price]")).toContainText(
    /€\s*234,56.*incl\. btw/,
  );
  const photo = article.getByRole("img", { name: foam.copy.title });
  await expect(photo).toHaveAttribute(
    "src",
    "/catalogue-images/nl/SET-FOAM/1/320",
  );
  await expect
    .poll(() => photo.evaluate((element) => element.naturalWidth))
    .toBe(1);
  await expect(
    page.getByRole("link", { name: "Admin", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: /bestel|winkelwagen|afrekenen/i }),
  ).toHaveCount(0);
  await page
    .getByRole("navigation", { name: "Winkelinformatie" })
    .getByRole("link", { name: "Retourneren", exact: true })
    .click();
  await expect(page).toHaveURL(`${origin}/nl/winkelinformatie#retourneren`);
  await expect(heading("Winkelinformatie")).toBeVisible();
  const returns = page.getByRole("region", { name: "Retourneren" });
  await expect(returns).toContainText(reviewContent.policies.returns);
  await expect(
    page.getByRole("link", { name: reviewContent.email }),
  ).toHaveAttribute("href", `mailto:${reviewContent.email}`);
  for (const name of [
    "Verkoper en contact",
    "Bezorging",
    "Privacy",
    "Betalen",
    "Verkoopvoorwaarden",
  ])
    await expect(heading(name)).toBeVisible();
  assert.ok(
    await page.evaluate(() => document.styleSheets.length > 0),
    "Review stylesheets should load",
  );
  await assertAccessibility(page);
}

export async function checkBrowser(origin, setState) {
  const browser = await chromium.launch();
  try {
    for (const width of [1200, 320]) {
      const context = await browser.newContext({
        viewport: { width, height: 900 },
        locale: "ar",
        serviceWorkers: "block",
      });
      try {
        await context.route("**/*", (route) =>
          new URL(route.request().url()).origin === origin
            ? route.continue()
            : route.abort(),
        );
        const page = await context.newPage();
        page.setDefaultTimeout(10000);
        await customerReads(page, origin);
        if (width === 320) {
          setState("unavailable");
          await page.goto(`${origin}/nl/collection`);
          await expect(
            page.getByRole("heading", {
              name: "Collectie tijdelijk niet beschikbaar",
            }),
          ).toBeVisible();
          setState("ready");
          await page.getByRole("button", { name: "Probeer opnieuw" }).click();
          await expect(
            page.getByRole("heading", { name: "De collectie" }),
          ).toBeVisible();
          await expect(
            page.getByRole("link", { name: covers.copy.title }),
          ).toBeVisible();
        }
      } finally {
        await context.close();
      }
    }
  } finally {
    await browser.close();
  }
}
