// Shared plan and identities for the isolated quote workload.
export const STEADY_OWNERS = 25;
export const BURST_OWNERS = 50;
export const STEADY_MS = 600_000;
export const STEADY_SLEEP_SECONDS = 10.15;
export const REQUEST_TIMEOUT_MS = 3_000;
export const MAX_REQUEST_INTERVAL_MS = 3_500;
export const MAX_STEADY_IDLE_MS = 11_500;
export const MIN_STEADY_REQUESTS = 40;

export function scenarios() {
  const result = {};
  for (let owner = 0; owner < STEADY_OWNERS; owner += 1) {
    result[`quote_steady_${owner}`] = {
      executor: "constant-vus",
      vus: 1,
      duration: "10m",
      gracefulStop: "4s",
      exec: "steadyQuote",
      env: { QUOTE_OWNER: String(owner) },
      tags: { workload: "steady" },
    };
  }
  result.quote_burst = {
    executor: "per-vu-iterations",
    vus: BURST_OWNERS,
    iterations: 1,
    startTime: "10m",
    maxDuration: "30s",
    exec: "burstQuote",
    tags: { workload: "burst" },
  };
  return result;
}

// These identifiers contain no credential or customer data.
export function quoteEffect(workload, owner, sequence) {
  if (
    !["steady", "burst"].includes(workload) ||
    !Number.isSafeInteger(owner) ||
    owner < 0 ||
    owner >= (workload === "steady" ? STEADY_OWNERS : BURST_OWNERS) ||
    !Number.isSafeInteger(sequence) ||
    sequence < 0 ||
    sequence > 9999 ||
    (workload === "burst" && sequence !== 0)
  ) {
    throw new Error("invalid synthetic quote position");
  }
  return `load-${workload}-${owner}-${sequence}`;
}
