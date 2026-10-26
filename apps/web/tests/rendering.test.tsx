import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { it, expect, vi } from "vitest";
import type { ReactNode } from "react";
const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
  usePathname: () => "/nl/products/courtyard-demo",
  useSearchParams: () => new URLSearchParams("variant=covers"),
  notFound: () => {
    throw new Error("NOT_FOUND");
  },
}));
vi.mock("next/link", () => ({
  default: ({
    href,
    children,
    ...props
  }: {
    href: string;
    children: ReactNode;
  }) => (
    <a href={href} {...props}>
      {children}
    </a>
  ),
}));
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "x-nonce": "test-nonce" }),
}));
vi.mock("server-only", () => ({}));
import { Preferences } from "../src/components/preferences";
import Gallery from "../src/app/[locale]/gallery/page";
import Admin from "../src/app/[locale]/admin/page";
import ErrorBoundary from "../src/app/[locale]/error";
import Loading from "../src/app/[locale]/loading";
it("loads explicit saved theme and switches language preserving query", async () => {
  localStorage.setItem("theme", "dark");
  render(<Preferences locale="nl" />);
  await waitFor(() =>
    expect(document.documentElement.dataset.theme).toBe("dark"),
  );
  fireEvent.change(screen.getByLabelText("Theme"), {
    target: { value: "light" },
  });
  expect(localStorage.getItem("theme")).toBe("light");
  fireEvent.change(screen.getByLabelText("Language"), {
    target: { value: "ar" },
  });
  expect(push).toHaveBeenCalledWith(
    "/ar/products/courtyard-demo?variant=covers",
  );
});
it("works when storage is denied", async () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new Error("denied");
  });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("denied");
  });
  render(<Preferences locale="nl" />);
  fireEvent.change(screen.getByLabelText("Theme"), {
    target: { value: "dark" },
  });
  await waitFor(() =>
    expect(document.documentElement.dataset.theme).toBe("dark"),
  );
});
it("covers gallery, unavailable admin, retry and loading", () => {
  const reset = vi.fn();
  render(
    <>
      <Gallery />
      <Admin />
      <ErrorBoundary reset={reset} />
      <Loading />
    </>,
  );
  expect(
    screen.getByRole("heading", { name: "Operations preview" }),
  ).toBeVisible();
  expect(screen.getByLabelText("Room width (cm)")).toHaveAccessibleDescription(
    "Enter a width greater than zero.",
  );
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(reset).toHaveBeenCalledOnce();
  expect(screen.getAllByRole("status")).toHaveLength(3);
});
import { renderToStaticMarkup } from "react-dom/server";
import Home from "../src/app/[locale]/page";
import ProductPage from "../src/app/[locale]/products/[slug]/page";
import Layout from "../src/app/[locale]/layout";
import robots from "../src/app/robots";
import sitemap from "../src/app/sitemap";
import { GET } from "../src/app/api/catalogue/[locale]/route";
import { loadCatalogue } from "../src/lib/load-catalogue";
const product = {
  sku: "DEMO-COURTYARD-COVERS",
  slug: "courtyard-demo",
  title: "Demonstration seating",
  description: "Not for sale",
  care: "Pending",
  minor_units: 12345,
  currency: "EUR",
  fill: "covers-only",
  fixture: true,
};
function serve(locale = "nl", items = [product]) {
  vi.stubEnv("APP_ENV", "test");
  vi.stubEnv("PREVIEW_TOKEN", "a".repeat(32));
  vi.stubEnv("API_URL", "http://127.0.0.1:4000");
  vi.stubEnv("CANONICAL_ORIGIN", "http://127.0.0.1:3000");
  vi.stubGlobal(
    "fetch",
    vi
      .fn()
      .mockImplementation(() =>
        Promise.resolve(
          Response.json({ items, locale, production_enabled: false }),
        ),
      ),
  );
}
it("renders real fixture facts in server HTML without advertising an offer", async () => {
  serve();
  const html = renderToStaticMarkup(
    await Home({ params: Promise.resolve({ locale: "nl" }) }),
  );
  expect(html).toContain("Demonstration seating");
  expect(html).toContain("Not for sale");
});
it("renders empty and Kurdish previews, details and missing routes", async () => {
  serve("kmr");
  expect(
    renderToStaticMarkup(
      await Home({ params: Promise.resolve({ locale: "ku" }) }),
    ),
  ).toContain("Sample price");
  serve("ar", []);
  expect(
    renderToStaticMarkup(
      await Home({ params: Promise.resolve({ locale: "ar" }) }),
    ),
  ).toContain("No demonstration items");
  serve();
  expect(
    renderToStaticMarkup(
      await ProductPage({
        params: Promise.resolve({ locale: "nl", slug: "courtyard-demo" }),
      }),
    ),
  ).toContain("DEMO-COURTYARD-COVERS");
  await expect(
    ProductPage({ params: Promise.resolve({ locale: "nl", slug: "missing" }) }),
  ).rejects.toThrow("NOT_FOUND");
  await expect(
    Home({ params: Promise.resolve({ locale: "bad" }) }),
  ).rejects.toThrow("NOT_FOUND");
  await expect(
    ProductPage({
      params: Promise.resolve({ locale: "bad", slug: "missing" }),
    }),
  ).rejects.toThrow("NOT_FOUND");
});
it("sets RTL language, nonce, noindex and empty sitemap on preview shells", async () => {
  const element = await Layout({
    params: Promise.resolve({ locale: "ar" }),
    children: <p>Fixture</p>,
  });
  const html = renderToStaticMarkup(element);
  expect(html).toContain('dir="rtl"');
  expect(html).toContain('lang="ar"');
  expect(html).toContain('nonce="test-nonce"');
  await expect(
    Layout({ params: Promise.resolve({ locale: "bad" }), children: null }),
  ).rejects.toThrow("NOT_FOUND");
  expect(robots()).toEqual({ rules: { userAgent: "*", allow: "/" } });
  expect(sitemap()).toEqual([]);
});
it("validates the same-origin proxy and redacts dependency failures", async () => {
  serve();
  const response = await GET(new Request("http://127.0.0.1/api/catalogue/nl"), {
    params: Promise.resolve({ locale: "nl" }),
  });
  expect(response.status).toBe(200);
  expect(response.headers.get("cache-control")).toBe("private, no-store");
  const invalid = await GET(new Request("http://127.0.0.1/api/catalogue/no"), {
    params: Promise.resolve({ locale: "no" }),
  });
  expect(invalid.status).toBe(400);
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue(new Response("unavailable", { status: 503 })),
  );
  await expect(loadCatalogue("nl")).rejects.toThrow("Catalogue unavailable");
  const failed = await GET(new Request("http://127.0.0.1/api/catalogue/nl"), {
    params: Promise.resolve({ locale: "nl" }),
  });
  expect(failed.status).toBe(503);
  expect(await failed.json()).toEqual({ code: "catalogue_unavailable" });
  serve("de");
  await expect(loadCatalogue("nl")).rejects.toThrow("locale mismatch");
});
