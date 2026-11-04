// Invented measurements for the visual control only.
export async function fillPlannerPreview(page) {
  for (const [id, value] of [
    ["plan-room-width", "430"],
    ["plan-room-depth", "315"],
    ["plan-items-1-width", "240"],
    ["plan-items-1-depth", "75"],
    ["plan-items-1-left", "18"],
    ["plan-items-1-top", "24"],
  ]) {
    await page.locator(`#${id}`).fill(value);
  }
  await page
    .locator('section[aria-labelledby="plan-doors"] select')
    .selectOption("none");
  await page.locator('#main form button[type="submit"]').click();
  await page.evaluate(() => window.scrollTo(0, 0));
}
