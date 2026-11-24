const web = "http://127.0.0.1:3000";
const api = "http://127.0.0.1:45671";

export async function stagingWebHealthy(env, request = fetch) {
  const storefrontChecks = env.STAGING_STORE_REVIEW_CONTENT
    ? [
        [`${web}/nl/collection`, 200],
        [`${web}/nl/winkelinformatie`, 200],
        [`${web}/nl/admin`, 404],
      ]
    : [
        [`${web}/nl`, env.STAGING_PUBLIC_STOREFRONT === "true" ? 200 : 401],
        [`${web}/nl/admin`, 401],
      ];
  const checks = [...storefrontChecks, [`${api}/health/ready`, 200]];
  const signal = AbortSignal.timeout(4000);
  try {
    const matches = await Promise.all(
      checks.map(async ([url, status]) => {
        const response = await request(url, { redirect: "manual", signal });
        await response.body?.cancel();
        return response.status === status;
      }),
    );
    return matches.every(Boolean);
  } catch {
    return false;
  }
}

if (import.meta.main && !(await stagingWebHealthy(process.env))) {
  process.exitCode = 1;
}
