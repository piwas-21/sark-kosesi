import { isLocale } from "../../../../../../lib/locale";
import { merchantCode } from "../../../../../../lib/published-validation";
import { configuration } from "../../../../../../lib/config";
import { readImage } from "../../../../../../lib/read-image";
export const dynamic = "force-dynamic";
const headers = {
  "Cache-Control": "private, no-store",
  "X-Robots-Tag": "noindex, nofollow",
  "X-Content-Type-Options": "nosniff",
};
export async function GET(
  _request: Request,
  {
    params,
  }: {
    params: Promise<{
      locale: string;
      sku: string;
      revision: string;
      edge: string;
    }>;
  },
) {
  const { locale, sku, revision, edge } = await params;
  if (
    !isLocale(locale) ||
    !merchantCode(sku) ||
    !/^[1-9]\d{0,9}$/.test(revision) ||
    Number(revision) > 2147483647 ||
    !["320", "640", "960"].includes(edge)
  )
    return new Response("Invalid image request", { status: 400, headers });
  try {
    const { api, token } = configuration();
    const response = await fetch(
      `${api}/api/published/${locale}/images/${sku}/${revision}/${edge}`,
      {
        headers: { Authorization: `Bearer ${token}` },
        cache: "no-store",
        redirect: "error",
        signal: AbortSignal.timeout(1500),
      },
    );
    if (response.status === 404)
      return new Response("Image unavailable", { status: 404, headers });
    if (!response.ok) throw new Error("Unavailable");
    const type = response.headers.get("content-type");
    if (type !== "image/jpeg" && type !== "image/png")
      throw new Error("Invalid image type");
    const bytes = await readImage(response);
    return new Response(bytes, {
      headers: { ...headers, "Content-Type": type },
    });
  } catch {
    return new Response("Image unavailable", { status: 503, headers });
  }
}
