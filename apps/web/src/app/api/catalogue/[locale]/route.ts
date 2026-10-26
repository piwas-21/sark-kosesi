import { isLocale } from "../../../../lib/locale";
import { loadCatalogue } from "../../../../lib/load-catalogue";
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ locale: string }> },
) {
  const { locale } = await params;
  const headers = {
    "Cache-Control": "private, no-store",
    "X-Robots-Tag": "noindex, nofollow",
  };
  if (!isLocale(locale))
    return Response.json(
      { code: "unsupported_locale" },
      { status: 400, headers },
    );
  try {
    return Response.json(await loadCatalogue(locale), { headers });
  } catch {
    return Response.json(
      { code: "catalogue_unavailable" },
      { status: 503, headers },
    );
  }
}
