import { isLocale } from "../../../lib/locale";
import { publishedCollection } from "../../../lib/load-published";
import { configuration } from "../../../lib/config";
import { catalogueFeed } from "../../../lib/published-presentation";
export const dynamic = "force-dynamic";
export async function GET(
  _request: Request,
  { params }: { params: Promise<{ locale: string }> },
) {
  const { locale } = await params;
  if (!isLocale(locale))
    return new Response("Unsupported locale", { status: 404 });
  try {
    const items = await publishedCollection(locale);
    return new Response(
      catalogueFeed(items, configuration().canonical, locale),
      {
        headers: {
          "Content-Type": "application/xml; charset=utf-8",
          "Cache-Control": "private, no-store",
          "X-Robots-Tag": "noindex, nofollow",
        },
      },
    );
  } catch {
    return new Response("Preview catalogue unavailable", {
      status: 503,
      headers: { "Cache-Control": "private, no-store" },
    });
  }
}
