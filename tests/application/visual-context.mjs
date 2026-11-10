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
export async function themeReady(page, theme, timeout = 30_000) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForFunction(
    (expected) => {
      const root = document.documentElement;
      if (root.dataset.theme !== expected) return false;
      const token = getComputedStyle(root).getPropertyValue("--ink").trim();
      if (!/^#[\da-f]{6}$/i.test(token)) return false;
      const rgb = [1, 3, 5].map((offset) =>
        parseInt(token.slice(offset, offset + 2), 16),
      );
      const ink = `rgb(${rgb.join(", ")})`;
      const heading = document.querySelector("main h1");
      const links = [
        ...document.querySelectorAll('header nav[aria-label="Main"] a'),
      ].filter((link) => link.getClientRects().length > 0);
      const mark = document.querySelector("header .brand svg path");
      return (
        getComputedStyle(document.body).color === ink &&
        heading !== null &&
        getComputedStyle(heading).color === ink &&
        links.length > 0 &&
        links.every((link) => getComputedStyle(link).color === ink) &&
        mark !== null &&
        getComputedStyle(mark).stroke === ink
      );
    },
    theme,
    { timeout },
  );
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
}
