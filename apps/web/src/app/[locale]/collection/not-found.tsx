import Link from "next/link";
import { configuration } from "../../../lib/config";

export default function UnavailableConfiguration() {
  if (configuration().storeReview) {
    return (
      <section>
        <h1>Product niet beschikbaar</h1>
        <p>Deze uitvoering is op dit moment niet in de collectie opgenomen.</p>
        <Link className="underline" href="/nl/collection">
          Bekijk de collectie
        </Link>
      </section>
    );
  }
  return (
    <section lang="en">
      <h1>Configuration unavailable</h1>
      <p>
        Product details are not available for this configuration in this
        language.
      </p>
      <p>Use Product preview above to choose another configuration.</p>
    </section>
  );
}
