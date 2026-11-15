import { spawn } from "node:child_process";

// Discovery effects run while the fixture server remains available. Never keep
// a pooled request open across a potentially long child process.
export function fetchWithoutReuse(url, options = {}) {
  const headers = new Headers(options.headers);
  headers.set("Connection", "close");
  return fetch(url, { ...options, headers });
}

// Output is deliberately discarded: fixture command failures remain redacted.
export function runFixtureProcess(command, args, timeoutMs) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      env: process.env,
      stdio: "ignore",
      detached: process.platform !== "win32",
    });
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      try {
        if (process.platform !== "win32" && child.pid)
          process.kill(-child.pid, "SIGKILL");
        else child.kill("SIGKILL");
      } catch {
        child.kill("SIGKILL");
      }
    }, timeoutMs);
    let settled = false;
    const finish = (success) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (success) resolve();
      else reject(new Error("Owned fixture effect failed; details withheld"));
    };
    child.once("error", () => finish(false));
    child.once("close", (code) => finish(code === 0 && !timedOut));
  });
}
