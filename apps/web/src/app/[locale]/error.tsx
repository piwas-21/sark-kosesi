"use client";
import { Button } from "@piwas/design";
import { useStoreReview } from "../../components/store-review-scope";
export default function ErrorBoundary({ reset }: { reset: () => void }) {
  if (useStoreReview()) {
    return (
      <section>
        <h1>Collectie tijdelijk niet beschikbaar</h1>
        <p role="alert">Probeer het over een ogenblik opnieuw.</p>
        <Button onClick={reset}>Probeer opnieuw</Button>
      </section>
    );
  }
  return (
    <section lang="en">
      <h1>Catalogue unavailable</h1>
      <p role="alert">The preview could not load. Try again shortly.</p>
      <Button onClick={reset}>Try again</Button>
    </section>
  );
}
