import http from "k6/http";
import { check } from "k6";
import { SharedArray } from "k6/data";
const budget = new SharedArray("budgets", () => [
  JSON.parse(open("../../config/performance-budgets.json")),
])[0];
export const options = {
  scenarios: {
    steady: {
      executor: "constant-vus",
      vus: budget.loadFixture.virtualUsers,
      duration: `${budget.loadFixture.steadyStateMinutes}m`,
    },
    burst: {
      executor: "shared-iterations",
      vus: budget.loadFixture.burstConcurrency,
      iterations: budget.loadFixture.burstConcurrency,
      startTime: `${budget.loadFixture.steadyStateMinutes}m`,
      maxDuration: "30s",
    },
  },
  thresholds: {
    "http_req_duration{scenario:burst}": [`p(95)<${budget.api.catalogueP95Ms}`],
    "http_req_failed{scenario:burst}": [`rate<=${budget.api.errorRateMaximum}`],
    http_req_duration: [`p(95)<${budget.api.catalogueP95Ms}`],
    http_req_failed: [`rate<=${budget.api.errorRateMaximum}`],
    checks: ["rate==1"],
  },
};
export default function () {
  const response = http.get(`${__ENV.API_URL}/api/catalogue/nl`, {
    headers: { Authorization: `Bearer ${__ENV.PREVIEW_TOKEN}` },
  });
  check(response, {
    "fixture read succeeds": (value) =>
      value.status === 200 &&
      value.json("items.0.sku") === "DEMO-COURTYARD-COVERS",
  });
}
