import { readdirSync } from "node:fs";
import { join } from "node:path";

const unitRoots = [
  { path: "apps/web/tests", suffix: ".test.tsx" },
  { path: "packages/design/tests", suffix: ".test.tsx" },
  { path: "packages/room-fit/tests", suffix: ".test.tsx" },
  { path: "packages/room-draft/tests", suffix: ".test.ts" },
];
function discoverUnitTests(roots = unitRoots) {
  return roots.flatMap(({ path, suffix }) => {
    const found = [];
    const walk = (dir) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const file = join(dir, entry.name);
        if (entry.isSymbolicLink())
          throw new Error(`Symlink in unit test inventory: ${file}`);
        if (entry.isDirectory()) walk(file);
        else if (entry.isFile() && entry.name.endsWith(suffix))
          found.push(file);
      }
    };
    walk(path);
    if (!found.length) throw new Error(`No unit tests found in ${path}`);
    return found.sort();
  });
}
export { discoverUnitTests, unitRoots };
