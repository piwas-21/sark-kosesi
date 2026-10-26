import "server-only";
import createClient from "openapi-fetch";
import type { paths } from "@piwas/contracts";
import { configuration } from "./config";
import { decodeCatalogue } from "./catalogue";
import type { Locale } from "./locale";
export async function loadCatalogue(locale: Locale) {
  const { api, token } = configuration();
  const client = createClient<paths>({
    baseUrl: api,
    headers: { Authorization: `Bearer ${token}` },
  });
  const { data, response } = await client.GET("/api/catalogue/{locale}", {
    params: { path: { locale } },
    cache: "no-store",
    signal: AbortSignal.timeout(1500),
  });
  if (!response.ok) throw new Error("Catalogue unavailable");
  const value: unknown = data;
  const catalogue = decodeCatalogue(value);
  if (catalogue.locale !== (locale === "ku" ? "kmr" : locale))
    throw new Error("Catalogue locale mismatch");
  return catalogue;
}
