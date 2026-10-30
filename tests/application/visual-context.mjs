// One capture/comparison environment; viewport changes alone do not reset every
// engine's screen/DPR defaults consistently.
export function visualContext(viewport, theme) {
  return {
    viewport,
    ...(theme ? { colorScheme: theme } : {}),
    screen: viewport,
    deviceScaleFactor: 1,
    reducedMotion: "reduce",
  };
}

// Wait for the selected theme's actual inherited text style and two paint frames.
// Selecting an option returns before React's theme effect/repaint necessarily ends.
export async function themeReady(page, theme) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForFunction((expected) => {
    const root = document.documentElement;
    if (root.dataset.theme !== expected) return false;
    const token = getComputedStyle(root).getPropertyValue("--ink").trim();
    if (!/^#[\da-f]{6}$/i.test(token)) return false;
    const rgb = [1, 3, 5].map((offset) =>
      parseInt(token.slice(offset, offset + 2), 16),
    );
    return getComputedStyle(document.body).color === `rgb(${rgb.join(", ")})`;
  }, theme);
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
}
