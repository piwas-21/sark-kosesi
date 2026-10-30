// Initial candidates for human review; this never updates or passes the visual gate.
import { chromium, firefox, webkit } from "@playwright/test";
import { mkdir, writeFile, readFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { visualContext, themeReady } from "./visual-context.mjs";
const base = process.env.WEB_BASE_URL;
const token = process.env.PREVIEW_TOKEN;
if (!base || !token) throw new Error("Owned app fixture required");
const directory = ".local/visual-candidates";
await mkdir(directory, { recursive: true });
const entries = [];
function sourceDigest() {
  return execFileSync(
    "python3",
    [
      "-c",
      "from visual_policy import source_digest; from pathlib import Path; print(source_digest(Path.cwd()))",
    ],
    { encoding: "utf8", env: { ...process.env, PYTHONPATH: "scripts" } },
  ).trim();
}
const sourceSha256 = sourceDigest();
const revision = execFileSync("git", ["rev-parse", "HEAD"], {
  encoding: "utf8",
}).trim();
const workingTreeClean =
  execFileSync("git", ["status", "--porcelain"], {
    encoding: "utf8",
  }).trim() === "";

for (const [name, type] of Object.entries({ chromium, firefox, webkit })) {
  const browser = await type.launch();
  try {
    for (const width of [1280, 390])
      for (const surface of [
        { name: "courtyard", path: "", locales: ["nl", "ar"] },
        {
          name: "gallery",
          path: "/gallery",
          locales: ["nl", "de", "fr", "en", "ar", "tr", "ku"],
        },
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
      ])
        for (const locale of surface.locales)
          for (const theme of ["light", "dark"]) {
            const context = await browser.newContext({
              ...visualContext(
                { width, height: width === 390 ? 844 : 900 },
                theme,
              ),
              httpCredentials: { username: "preview", password: token },
            });
            const page = await context.newPage();
            await page.goto(`${base}/${locale}${surface.path}`);
            await page.getByLabel("Theme").selectOption(theme);
            if (surface.name === "gallery")
              await page.getByLabel("Room width (cm)").focus();
            await themeReady(page, theme);
            const file = `${name}-${surface.name}-${locale}-${theme}-${width}.png`;
            await page.screenshot({
              path: `${directory}/${file}`,
              fullPage: true,
              animations: "disabled",
            });
            entries.push(file);
            await context.close();
          }
  } finally {
    await browser.close();
  }
}
await writeFile(
  `${directory}/index.html`,
  `<!doctype html><meta charset="utf-8"><title>P2 visual review candidates</title><style>body{font:16px system-ui;margin:2rem}main{display:grid;grid-template-columns:repeat(auto-fit,minmax(300px,1fr));gap:2rem}img{width:100%;height:400px;object-fit:contain;object-position:top;border:1px solid}a{color:inherit}</style><h1>P2 visual review candidates</h1><p>Private, non-selling application. Draft locales, invented fixtures. These are candidates, not approved screenshot baselines. All three browsers, desktop/phone, NL/Arabic home, collection, covers/filled configurations, missing products, seven-locale gallery and empty English/Kurmanji collection, light/dark.</p><main>${entries.map((file) => `<figure><a href="${file}"><img src="${file}" alt="${file}" loading="lazy"></a><figcaption>${file}</figcaption></figure>`).join("")}</main>`,
);
console.log(
  "Initial visual candidates captured; normal visual tests still require approved baselines.",
);

if (sourceDigest() !== sourceSha256)
  throw new Error("Source changed during candidate capture");
const files = await Promise.all(
  entries.map(async (name) => {
    const bytes = await readFile(`${directory}/${name}`);
    return {
      name,
      sha256: createHash("sha256").update(bytes).digest("hex"),
      bytes: bytes.byteLength,
    };
  }),
);
await writeFile(
  `${directory}/manifest.json`,
  JSON.stringify(
    {
      schemaVersion: 1,
      revision,
      workingTreeClean,
      sourceSha256,
      platform: process.platform,
      capturedAt: new Date().toISOString(),
      humanApproved: false,
      productionEnabled: false,
      files,
    },
    null,
    2,
  ),
);
