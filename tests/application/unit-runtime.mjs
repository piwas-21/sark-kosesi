import { describe, it as nodeIt, afterEach } from "node:test";
import { format } from "node:util";
import {
  JestChaiExpect,
  JestExtend,
  JestAsymmetricMatchers,
  GLOBAL_EXPECT,
  setState,
  chai,
} from "@vitest/expect";
import * as matchers from "@testing-library/jest-dom/matchers";
import { fn, spyOn, restoreAllMocks } from "@vitest/spy";

function createRuntime() {
  chai.use(JestExtend);
  chai.use(JestChaiExpect);
  chai.use(JestAsymmetricMatchers);
  const expect = (value) => chai.expect(value);
  Object.assign(
    expect,
    chai.expect,
    globalThis[Symbol.for("asymmetric-matchers-object")],
  );
  expect.extend = (values) => chai.expect.extend(expect, values);
  globalThis[GLOBAL_EXPECT] = expect;
  setState({ assertionCalls: 0 }, expect);
  expect.extend(matchers);

  const it = Object.assign(nodeIt, {
    each: (values) => (name, callback) => {
      for (const value of values) {
        const args = Array.isArray(value) ? value : [value];
        nodeIt(format(name, ...args), () => callback(...args));
      }
    },
  });
  const env = new Map();
  const globals = new Map();
  const vi = {
    fn,
    spyOn,
    restoreAllMocks,
    mocked: (value) => value,
    stubEnv: (name, value) => {
      if (!env.has(name)) env.set(name, process.env[name]);
      process.env[name] = value;
    },
    unstubAllEnvs: () => {
      for (const [name, value] of env) {
        if (value === undefined) delete process.env[name];
        else process.env[name] = value;
      }
      env.clear();
    },
    stubGlobal: (name, value) => {
      if (!globals.has(name))
        globals.set(name, Object.getOwnPropertyDescriptor(globalThis, name));
      Object.defineProperty(globalThis, name, {
        value,
        configurable: true,
        writable: true,
      });
    },
    unstubAllGlobals: () => {
      for (const [name, descriptor] of globals) {
        if (descriptor) Object.defineProperty(globalThis, name, descriptor);
        else Reflect.deleteProperty(globalThis, name);
      }
      globals.clear();
    },
  };
  const routerPush = fn();
  return { afterEach, describe, expect, it, routerPush, vi };
}
const runtimeKey = Symbol.for("piwas.unit.runtime");
const runtime = (globalThis[runtimeKey] ??= createRuntime());
const { expect, it, routerPush, vi } = runtime;
export { afterEach, describe, expect, it, routerPush, vi };
