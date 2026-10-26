import { NextResponse, type NextRequest } from "next/server";
import { randomBytes, timingSafeEqual } from "node:crypto";
import { configuration } from "./lib/config";
import { resolveLocale } from "./lib/locale";
export function proxy(request: NextRequest) {
  const { token, canonical } = configuration();
  const expected = Buffer.from(
    `Basic ${Buffer.from(`preview:${token}`).toString("base64")}`,
  );
  const supplied = Buffer.from(request.headers.get("authorization") ?? "");
  if (
    supplied.length !== expected.length ||
    !timingSafeEqual(supplied, expected)
  ) {
    return new NextResponse("Private preview", {
      status: 401,
      headers: {
        "WWW-Authenticate": 'Basic realm="Piwas preview", charset="UTF-8"',
        "Cache-Control": "private, no-store",
        "X-Robots-Tag": "noindex, nofollow",
      },
    });
  }
  if (request.nextUrl.pathname === "/") {
    const locale = resolveLocale(
      request.cookies.get("locale")?.value,
      request.headers.get("accept-language") ?? "",
    );
    const target = new URL(`/${locale}`, canonical);
    target.search = request.nextUrl.search;
    const response = NextResponse.redirect(target);
    response.headers.set("Cache-Control", "private, no-store");
    response.headers.set("Vary", "Cookie, Accept-Language");
    return response;
  }
  const nonce = randomBytes(18).toString("base64");
  const policy = `default-src 'self'; script-src 'self' 'nonce-${nonce}' 'strict-dynamic'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'`;
  const headers = new Headers(request.headers);
  headers.set("x-nonce", nonce);
  headers.set("Content-Security-Policy", policy);
  const response = NextResponse.next({ request: { headers } });
  response.headers.set("Content-Security-Policy", policy);
  response.headers.set("Cache-Control", "private, no-store");
  response.headers.set("X-Robots-Tag", "noindex, nofollow");
  response.headers.set("X-Content-Type-Options", "nosniff");
  response.headers.set("Referrer-Policy", "no-referrer");
  return response;
}
export const config = {
  matcher: ["/((?!_next/static|_next/image|favicon.ico).*)"],
};
