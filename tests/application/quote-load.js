import http from "k6/http";
import exec from "k6/execution";
import { sleep } from "k6";
import { SharedArray } from "k6/data";
import {
  BURST_OWNERS,
  STEADY_OWNERS,
  STEADY_MS,
  STEADY_SLEEP_SECONDS,
  REQUEST_TIMEOUT_MS,
  quoteEffect,
  scenarios,
} from "./quote-load-contract.mjs";

const fixture = new SharedArray("owned quote fixture", () => {
  const custody = JSON.parse(open(__ENV.QUOTE_LOAD_FIXTURE));
  if (
    !/^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}$/.test(custody.url) ||
    custody.origin !== "https://guest.example.test" ||
    !/^[0-9a-f]{64}$/.test(custody.bearer) ||
    !Array.isArray(custody.owners) ||
    custody.owners.length !== STEADY_OWNERS + BURST_OWNERS ||
    !custody.owners.every(
      ({ cookie, csrf }) =>
        /^__Host-piwas-guest=[A-Za-z0-9_-]{22}$/.test(cookie) &&
        /^[0-9a-f]{64}$/.test(csrf),
    )
  ) {
    throw new Error("invalid owned quote fixture");
  }
  return [custody];
})[0];

export const options = {
  scenarios: scenarios(),
  maxRedirects: 0,
  noCookiesReset: false,
  systemTags: ["scenario", "status", "method", "expected_response"],
};

function request(workload, owner, sequence) {
  const effect = quoteEffect(workload, owner, sequence);
  const index = workload === "steady" ? owner : STEADY_OWNERS + owner;
  const { cookie, csrf } = fixture.owners[index];
  const body = JSON.stringify({
    expected_basket_revision: 1,
    effect_key: effect,
    country: "NL",
    postcode: "1234 AB",
  });
  const params = {
    headers: {
      Authorization: `Bearer ${fixture.bearer}`,
      Cookie: cookie,
      Origin: fixture.origin,
      "Content-Type": "application/json",
      "x-csrf-token": csrf,
    },
    timeout: `${REQUEST_TIMEOUT_MS}ms`,
  };
  const started = Date.now();
  const response = http.post(
    `${fixture.url}/api/guest/nl/quotes`,
    body,
    params,
  );
  const finished = Date.now();
  let replayed = null;
  let quote = null;
  if (response.status === 200) {
    let value = null;
    try {
      value = response.json();
    } catch {
      // Report a failed projection without recording the raw response.
    }
    const item =
      value?.quote?.lines?.length === 1 ? value.quote.lines[0] : null;
    if (
      item &&
      value.quote.basket_revision === 1 &&
      typeof value.replayed === "boolean"
    ) {
      replayed = value.replayed;
      quote = {
        sku: item.sku,
        quantity: item.quantity,
        locale: value.quote.locale,
        currency: value.quote.currency,
        indicative: value.quote.indicative,
        total: {
          gross_minor: value.quote.total?.gross_minor,
          net_minor: value.quote.total?.net_minor,
          vat_minor: value.quote.total?.vat_minor,
        },
      };
      // Emit only the exact known synthetic projection, even for malformed values.
      if (
        quote.sku !== "SET-FILLED" ||
        quote.quantity !== 3 ||
        quote.locale !== "nl" ||
        quote.currency !== "EUR" ||
        quote.indicative !== true ||
        quote.total.gross_minor !== 38245 ||
        quote.total.net_minor !== 31607 ||
        quote.total.vat_minor !== 6638
      ) {
        quote = null;
      }
    }
  }
  console.log(
    JSON.stringify({
      scenario_start_ms: exec.scenario.startTime,
      request: {
        workload,
        owner,
        sequence,
        started_ms: started,
        finished_ms: finished,
        status: response.status,
        replayed,
        quote,
      },
    }),
  );
}

export function steadyQuote() {
  request("steady", Number(__ENV.QUOTE_OWNER), exec.scenario.iterationInTest);
  const remaining = exec.scenario.startTime + STEADY_MS - Date.now();
  sleep(Math.min(STEADY_SLEEP_SECONDS, Math.max(0, remaining) / 1000 + 0.1));
}

export function burstQuote() {
  // Wait for the steady window to close across k6 scenario startup offsets.
  sleep(0.25);
  request("burst", exec.scenario.iterationInTest, 0);
}

export function handleSummary(data) {
  return {
    stdout: JSON.stringify({
      requests: data.metrics.http_reqs?.values.count,
      dropped: data.metrics.dropped_iterations?.values.count ?? 0,
      duration_ms: data.state.testRunDurationMs,
    }),
  };
}
