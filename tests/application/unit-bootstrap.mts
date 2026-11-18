import { afterEach, mock } from "node:test";
import { createRequire, registerHooks } from "node:module";
import { pathToFileURL } from "node:url";
import { JSDOM } from "jsdom";
import { fn } from "@vitest/spy";
import { discoverUnitTests } from "./unit-inventory.mjs";

const dom = new JSDOM("<!doctype html><html><body></body></html>", {
  url: "http://localhost/",
  pretendToBeVisual: true,
});
const browserGlobals = new Set([
  "Blob",
  "File",
  "FileReader",
  "Storage",
  "HTMLElement",
  "Node",
]);
for (const name of Object.getOwnPropertyNames(dom.window)) {
  if (!(name in globalThis) || browserGlobals.has(name)) {
    Object.defineProperty(globalThis, name, {
      configurable: true,
      get: () => Reflect.get(dom.window, name),
      set: (value: unknown) => Reflect.set(dom.window, name, value),
    });
  }
}
Object.defineProperty(globalThis, "requestAnimationFrame", {
  value: dom.window.requestAnimationFrame.bind(dom.window),
  configurable: true,
  writable: true,
});
Object.defineProperty(globalThis, "cancelAnimationFrame", {
  value: dom.window.cancelAnimationFrame.bind(dom.window),
  configurable: true,
  writable: true,
});
Object.defineProperty(globalThis, "window", {
  value: dom.window,
  configurable: true,
});
Object.defineProperty(globalThis, "document", {
  value: dom.window.document,
  configurable: true,
});
Object.defineProperty(globalThis, "localStorage", {
  value: dom.window.localStorage,
  configurable: true,
});
Object.defineProperty(globalThis, "matchMedia", {
  configurable: true,
  writable: true,
  value: fn().mockImplementation(() => ({
    matches: false,
    addEventListener: fn(),
    removeEventListener: fn(),
  })),
});

const { routerPush, vi } = await import("./unit-runtime.mjs");

// Components import CSS modules for class names; no CSS runs in jsdom.
const require = createRequire(import.meta.url);
require.extensions[".css"] = (module) => {
  module.exports = new Proxy(
    {},
    { get: (_, key) => (key === "__esModule" ? false : key) },
  );
};
registerHooks({
  load(url, context, nextLoad) {
    if (url.endsWith(".css")) {
      return {
        format: "module",
        shortCircuit: true,
        source: "export default new Proxy({}, {get: (_, key) => key})",
      };
    }
    return nextLoad(url, context);
  },
});

const target = process.env.SARK_UNIT_FILE;
if (!target || !discoverUnitTests().includes(target)) {
  throw new Error("Missing or invalid unit test target");
}
if (
  [
    "apps/web/tests/published-loader.test.tsx",
    "apps/web/tests/published-rendering.test.tsx",
    "apps/web/tests/rendering.test.tsx",
  ].includes(target)
) {
  mock.module("server-only", { exports: {} });
}
if (target === "apps/web/tests/published-rendering.test.tsx") {
  mock.module("next/navigation", {
    exports: {
      notFound: () => {
        throw new Error("NOT_FOUND");
      },
      redirect: (url: string) => {
        throw new Error(`REDIRECT:${url}`);
      },
    },
  });
  mock.module(
    pathToFileURL(`${process.cwd()}/apps/web/src/lib/load-published.ts`),
    {
      exports: { publishedPage: fn(), publishedCollection: fn() },
    },
  );
}
if (target === "apps/web/tests/rendering.test.tsx") {
  const React = await import("react");
  mock.module("next/font/local", {
    exports: { default: () => ({ variable: "fixture-font" }) },
  });
  mock.module("next/navigation", {
    exports: {
      useRouter: () => ({ push: routerPush }),
      usePathname: () => "/nl/products/courtyard-demo",
      useSearchParams: () => new URLSearchParams("variant=covers"),
      notFound: () => {
        throw new Error("NOT_FOUND");
      },
    },
  });
  mock.module("next/link", {
    exports: {
      default: ({ children, ...props }: { children: React.ReactNode }) =>
        React.createElement("a", props, children),
    },
  });
  mock.module("next/headers", {
    exports: { headers: async () => new Headers({ "x-nonce": "test-nonce" }) },
  });
}
const { cleanup } = await import("@testing-library/react");
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  localStorage.clear();
});
await import(pathToFileURL(`${process.cwd()}/${target}`).href);
