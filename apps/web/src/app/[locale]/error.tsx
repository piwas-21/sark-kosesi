"use client";
import { Button } from "@piwas/design";
export default function ErrorBoundary({ reset }: { reset: () => void }) {
  return (
    <section lang="en">
      <h1>Catalogue unavailable</h1>
      <p role="alert">The preview could not load. Try again shortly.</p>
      <Button onClick={reset}>Try again</Button>
    </section>
  );
}
