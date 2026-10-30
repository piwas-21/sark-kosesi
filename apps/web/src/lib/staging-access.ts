import { locales } from "./locale";

const locale = `(?:${locales.join("|")})`;
const publicRoutes = [
  /^\/$/,
  /^\/(?:robots\.txt|sitemap\.xml)$/,
  new RegExp(`^/${locale}(?:/(?:gallery|collection|catalogue\\.xml))?$`),
  new RegExp(`^/${locale}/(?:products|collection)/[a-z0-9]+(?:-[a-z0-9]+)*$`),
  new RegExp(`^/api/catalogue/${locale}$`),
  new RegExp(
    `^/catalogue-images/${locale}/[A-Z0-9-]+/[1-9][0-9]*/(?:320|640|960)$`,
  ),
];

// Unknown/future routes and every mutation retain the browser credential gate.
export function publicStagingRead(path: string, method: string): boolean {
  return (
    (method === "GET" || method === "HEAD") &&
    publicRoutes.some((route) => route.test(path))
  );
}
