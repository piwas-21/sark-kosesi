import { test, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const runtimeFailures = new WeakMap();
test.beforeEach(async ({ page }) => {
  const errors = [];
  runtimeFailures.set(page, errors);
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*', route => {
    const url = new URL(route.request().url());
    if (url.hostname !== '127.0.0.1') {
      errors.push(`Unexpected external request: ${url.origin}`);
      return route.abort();
    }
    return route.continue();
  });
});
test.afterEach(async ({ page }) => expect(runtimeFailures.get(page)).toEqual([]));

for (const locale of ['nl', 'de', 'fr', 'en', 'ar', 'tr', 'kmr']) {
  test(`phone layout and locale: ${locale}`, async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto(`/index.html?embed=1&lang=${locale}&theme=light#home`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale === 'kmr' ? 'ku-Latn' : locale);
    await expect(page.locator('html')).toHaveAttribute('dir', locale === 'ar' ? 'rtl' : 'ltr');
    await expect(page.locator('#home-view h1')).toBeVisible();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > document.documentElement.clientWidth);
    expect(overflow).toBe(false);
  });
}

test.describe('device defaults', () => {
  test.use({ locale: 'tr-TR', colorScheme: 'dark' });
  test('language and theme apply without a saved choice', async ({ page }) => {
    await page.goto('/index.html?embed=1');
    await expect(page.locator('html')).toHaveAttribute('lang', 'tr');
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  });
});

test('covers-only basket has the independent expected amount and content', async ({ page }) => {
  await page.goto('/index.html?embed=1&lang=en&theme=light#product');
  await page.getByText('Covers only', { exact: true }).click();
  await expect(page.locator('#product-price')).toHaveText('€189');
  await expect(page.locator('#bundle-summary')).toHaveText('Sample bundle: 2 seat covers, 3 back covers and 2 bolster covers. All arrive empty.');
  await page.getByRole('button', { name: 'Add to sample basket', exact: true }).click();
  await expect(page.locator('#basket')).toBeVisible();
  await expect(page.locator('.bag')).toHaveAccessibleName('Your sample basket: 1');
  await expect(page.locator('#basket-content')).toContainText('180 × 70 cm · Covers only · €189');
  await expect(page.locator('.basket-total')).toContainText('€189');
  await expect(page.locator('#basket-content')).toContainText('Checkout is not connected');
});

test('rectangle fit distinguishes too-wide from rotated fit', async ({ page }) => {
  await page.goto('/index.html?embed=1&lang=en&theme=light#planner');
  await page.getByRole('slider', { name: 'Wall width', exact: true }).press('Home');
  await expect(page.locator('#fit-result')).toContainText('too large');
  await page.getByRole('button', { name: 'Rotate seating 90°', exact: true }).click();
  await expect(page.locator('#fit-result')).toContainText('Space remaining: 80 × 100 cm');
});

test('reduced motion disables ornament and preserves disclosure', async ({ page }) => {
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/index.html?embed=1&lang=en#home');
  await expect(page.locator('#motion-toggle')).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.orbit-type')).toHaveCSS('animation-name', 'none');
  await page.getByRole('button', { name: 'Explore the textile', exact: true }).click();
  await expect(page.locator('#textile-detail')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(page.locator('#textile-detail')).toBeHidden();
  await expect(page.locator('#textile-hotspot')).toBeFocused();
});

test('product controls have no serious or critical accessibility findings', async ({ page }) => {
  await page.goto('/index.html?embed=1&lang=en&theme=light#product');
  await expect(page.locator('#product-view')).toHaveCSS('opacity', '1');
  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations.filter(v => ['serious', 'critical'].includes(v.impact))).toEqual([]);
});

test('prototype stays noindex and studio routes to a real screen', async ({ page }) => {
  const response = await page.goto('/studio.html?lang=en&theme=light');
  expect(response.headers()['x-robots-tag']).toBe('noindex, nofollow');
  await expect(page.locator('meta[name="robots"]')).toHaveAttribute('content', 'noindex,nofollow');
  await page.getByRole('button', { name: 'The set', exact: true }).click();
  await expect(page.frameLocator('#preview').getByRole('heading', { name: 'The gathering set', exact: true })).toBeVisible();
});
