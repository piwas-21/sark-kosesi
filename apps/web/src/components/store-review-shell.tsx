import { Suspense, type ReactNode } from "react";
import Link from "next/link";
import { Preferences } from "./preferences";
import { StoreReviewScope } from "./store-review-scope";
import { CourtyardMark } from "@piwas/design";
import { storeInformationPath, type StoreReview } from "../lib/store-review";

export function StoreReviewShell({
  content,
  children,
}: {
  content: StoreReview;
  children: ReactNode;
}) {
  return (
    <>
      <a href="#main" className="skip">
        Naar de hoofdinhoud
      </a>
      <div className="shell">
        <header>
          <Link
            className="brand max-w-full wrap-anywhere"
            href="/nl/collection"
          >
            <CourtyardMark />
            {content.storeName}
          </Link>
          <nav aria-label="Hoofdnavigatie">
            <Link className="underline" href="/nl/collection">
              Collectie
            </Link>
            <Link className="underline" href={storeInformationPath}>
              Winkelinformatie
            </Link>
          </nav>
          <Suspense>
            <Preferences locale="nl" storeReview />
          </Suspense>
        </header>
        <main id="main">
          <StoreReviewScope>{children}</StoreReviewScope>
        </main>
        <footer>
          <p>{content.sellerName}</p>
          <nav className="mt-4" aria-label="Winkelinformatie">
            <Link className="underline" href={storeInformationPath}>
              Contact
            </Link>
            <Link
              className="underline"
              href={`${storeInformationPath}#bezorging`}
            >
              Bezorging
            </Link>
            <Link
              className="underline"
              href={`${storeInformationPath}#retourneren`}
            >
              Retourneren
            </Link>
            <Link
              className="underline"
              href={`${storeInformationPath}#privacy`}
            >
              Privacy
            </Link>
            <Link
              className="underline"
              href={`${storeInformationPath}#betalen`}
            >
              Betalen
            </Link>
            <Link
              className="underline"
              href={`${storeInformationPath}#voorwaarden`}
            >
              Verkoopvoorwaarden
            </Link>
          </nav>
        </footer>
      </div>
    </>
  );
}
