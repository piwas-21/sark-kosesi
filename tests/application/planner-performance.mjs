export async function checkPlannerInteraction(page, timeout = 5000) {
  await page.locator("#plan-room-width").fill("400");
  // The unit locks only after React handles the input; a static shell cannot pass.
  await page.locator("#plan-unit:disabled").waitFor({ timeout });
  await page.locator("#plan-room-depth").fill("300");
  await page.locator("#plan-items-1-width").fill("150");
  await page.locator("#plan-items-1-depth").fill("70");
  await page.getByLabel("Welke ruimte moet vrij blijven?").selectOption("none");
  await page.getByRole("button", { name: "Indeling controleren" }).click();
  await page.locator('[role="status"][data-state="fit"]').waitFor({ timeout });
  await page.locator("#plan-items-1-left").fill("390");
  await page.getByRole("button", { name: "Indeling controleren" }).click();
  await page
    .locator('[role="status"][data-state="conflict"]')
    .waitFor({ timeout });
}
