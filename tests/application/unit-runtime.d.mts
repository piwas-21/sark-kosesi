import type { ExpectStatic } from "@vitest/expect";
import type { MaybeMocked } from "@vitest/spy";
import { fn, spyOn, restoreAllMocks } from "@vitest/spy";
import { describe, it as nodeIt, afterEach } from "node:test";
import type { TestingLibraryMatchers } from "@testing-library/jest-dom/matchers";
declare module "@vitest/expect" {
  interface Assertion<R, T> extends TestingLibraryMatchers<R, T> {}
}
type Each = <T>(
  values: readonly T[],
) => (
  name: string,
  callback: (
    ...values: T extends readonly unknown[] ? [...T] : [T]
  ) => void | Promise<void>,
) => void;
declare const it: typeof nodeIt & { each: Each };
declare const expect: ExpectStatic;
declare const routerPush: ReturnType<typeof fn>;
declare const vi: {
  fn: typeof fn;
  spyOn: typeof spyOn;
  restoreAllMocks: typeof restoreAllMocks;
  mocked<T>(value: T): MaybeMocked<T>;
  stubEnv(name: string, value: string): void;
  unstubAllEnvs(): void;
  stubGlobal(name: string, value: unknown): void;
  unstubAllGlobals(): void;
};
export { afterEach, describe, expect, it, routerPush, vi };
