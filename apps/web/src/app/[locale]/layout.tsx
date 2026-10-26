import type { Metadata } from "next";
import { headers } from "next/headers";
import { notFound } from "next/navigation";
import { Suspense, type ReactNode } from "react";
import Link from "next/link";
import { CourtyardMark } from "@piwas/design";
import { Preferences } from "../../components/preferences";
import { direction, isLocale, languageTag } from "../../lib/locale";
import { themeBootstrap } from "../../lib/theme";
import "../globals.css";
export const dynamic = "force-dynamic";
export const metadata: Metadata = {
  title: "Dicle design preview",
  description:
    "Private Piwas application preview. All catalogue values are test fixtures.",
  robots: { index: false, follow: false },
};
export default async function Layout({
  children,
  params,
}: {
  children: ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  if (!isLocale(locale)) notFound();
  const nonce = (await headers()).get("x-nonce") ?? undefined;
  return (
    <html
      lang={languageTag(locale)}
      dir={direction(locale)}
      suppressHydrationWarning
    >
      <head>
        <script
          nonce={nonce}
          dangerouslySetInnerHTML={{ __html: themeBootstrap }}
        />
      </head>
      <body>
        <a href="#main" className="skip">
          Skip to content
        </a>
        <p className="notice" lang="en">
          Private preview · Draft translations · Test fixtures only · Nothing is
          for sale
        </p>
        <div className="shell">
          <header>
            <Link className="brand" href={`/${locale}`}>
              <CourtyardMark />
              dicle.
            </Link>
            <nav aria-label="Main" lang="en">
              <Link href={`/${locale}#collection`}>Collection</Link>
              <Link href={`/${locale}/gallery`}>Components</Link>
              <Link href={`/${locale}/admin`}>Admin</Link>
            </nav>
            <Suspense>
              <Preferences locale={locale} />
            </Suspense>
          </header>
          <main id="main">{children}</main>
          <footer lang="en">
            Piwas application preview. Dicle is a working name. Product and
            language review pending.
          </footer>
        </div>
      </body>
    </html>
  );
}
