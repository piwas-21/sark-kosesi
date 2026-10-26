export function configuration(
  values: Record<string, string | undefined> = process.env,
) {
  if (values.APP_ENV !== "local" && values.APP_ENV !== "test")
    throw new Error("Production is disabled");
  const token = values.PREVIEW_TOKEN;
  if (!token || !/^[A-Za-z0-9_-]{32,128}$/.test(token))
    throw new Error("Invalid preview configuration");
  const api = new URL(values.API_URL ?? "");
  const canonical = new URL(values.CANONICAL_ORIGIN ?? "");
  for (const address of [api, canonical]) {
    if (
      address.protocol !== "http:" ||
      address.hostname !== "127.0.0.1" ||
      address.username ||
      address.password ||
      address.pathname !== "/" ||
      address.search ||
      address.hash
    )
      throw new Error("P1 requires loopback origins");
  }
  return { token, api: api.origin, canonical: canonical.origin };
}
