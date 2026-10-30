export function configuration(
  values: Record<string, string | undefined> = process.env,
) {
  const staging = values.APP_ENV === "staging";
  if (!staging && values.APP_ENV !== "local" && values.APP_ENV !== "test")
    throw new Error("Production is disabled");
  const token = values.PREVIEW_TOKEN;
  if (!token || !/^[A-Za-z0-9_-]{32,128}$/.test(token))
    throw new Error("Invalid preview configuration");
  const revision = values.PREVIEW_REVISION;
  if (staging && (!revision || !/^[a-f0-9]{40}$/.test(revision)))
    throw new Error("Staging requires an exact source revision");
  const publicSetting = values.STAGING_PUBLIC_STOREFRONT;
  if (
    publicSetting !== undefined &&
    publicSetting !== "false" &&
    (publicSetting !== "true" || !staging)
  )
    throw new Error("Invalid public staging configuration");
  const publicStorefront = staging && publicSetting === "true";
  const adminToken = values.STAGING_ADMIN_TOKEN ?? "";
  if (
    publicStorefront &&
    (!/^[A-Za-z0-9_-]{32,128}$/.test(adminToken) || adminToken === token)
  )
    throw new Error("Public staging requires a separate admin credential");
  const api = new URL(values.API_URL ?? "");
  const canonical = new URL(values.CANONICAL_ORIGIN ?? "");
  for (const address of [api, canonical]) {
    if (
      address.username ||
      address.password ||
      address.pathname !== "/" ||
      address.search ||
      address.hash
    )
      throw new Error("Invalid preview origin");
  }
  if (api.protocol !== "http:" || api.hostname !== "127.0.0.1")
    throw new Error("Preview API requires a loopback origin");
  if (staging) {
    if (
      canonical.protocol !== "https:" ||
      canonical.port ||
      !/^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,63}$/.test(
        canonical.hostname,
      )
    )
      throw new Error("Staging requires an HTTPS DNS origin");
  } else if (
    canonical.protocol !== "http:" ||
    canonical.hostname !== "127.0.0.1"
  ) {
    throw new Error("Local/test previews require a loopback origin");
  }
  return {
    token,
    api: api.origin,
    canonical: canonical.origin,
    revision: staging ? revision : undefined,
    publicStorefront,
    browserUsername: publicStorefront ? "test-admin" : "preview",
    browserToken: publicStorefront ? adminToken : token,
  };
}
