import type { PublishedProduct } from "../lib/published-catalogue";
import styles from "./published.module.css";
import Link from "next/link";
import { storeInformationPath, storeReviewFill } from "../lib/store-review";
function dimensions(d: PublishedProduct["footprint"]) {
  return `${d.width_mm} × ${d.depth_mm} × ${d.height_mm} mm (± ${d.tolerance_mm} mm)`;
}
export function ProductFacts({
  product: p,
  storeReview = false,
}: {
  product: PublishedProduct;
  storeReview?: boolean;
}) {
  const uiLanguage = storeReview ? "nl" : "en";

  return (
    <div className={styles.facts}>
      <section aria-labelledby="included-title">
        <h2 id="included-title" lang={uiLanguage}>
          {storeReview ? "Wat zit er in de set?" : "What is included"}
        </h2>
        <dl lang={uiLanguage}>
          <dt>SKU</dt>
          <dd>
            <bdi>{p.sku}</bdi>
          </dd>
          <dt>{storeReview ? "Uitvoering" : "Configuration"}</dt>
          <dd>{storeReview ? storeReviewFill(p.fill) : p.fill}</dd>
          <dt>{storeReview ? "Maat" : "Size"}</dt>
          <dd>{p.size_code}</dd>
          <dt>{storeReview ? "Stof" : "Textile"}</dt>
          <dd>{p.textile_code}</dd>
          <dt>
            {storeReview ? "Afmetingen opgesteld" : "Assembled footprint"}
          </dt>
          <dd>
            <bdi>{dimensions(p.footprint)}</bdi>
          </dd>
        </dl>
        {p.components.map((c) => (
          <section key={c.code} lang={uiLanguage}>
            <h3>
              <bdi>{c.code}</bdi>
            </h3>
            <dl>
              <dt>{storeReview ? "Aantal" : "Quantity"}</dt>
              <dd>{c.quantity}</dd>
              <dt>{storeReview ? "Afmetingen" : "Measurements"}</dt>
              <dd>
                <bdi>{dimensions(c.dimensions)}</bdi>
              </dd>
              {c.material.kind === "cover" ? (
                <>
                  <dt>
                    {storeReview ? "Vezelsamenstelling" : "Fibre composition"}
                  </dt>
                  <dd>
                    {c.material.fibres.map((f) => (
                      <span key={f.name}>
                        {f.name}: <bdi>{f.basis_points / 100}%</bdi>
                        <br />
                      </span>
                    ))}
                  </dd>
                </>
              ) : (
                <>
                  <dt>{storeReview ? "Schuimdichtheid" : "Foam density"}</dt>
                  <dd>
                    <bdi>{c.material.density_g_per_m3} g/m³</bdi>
                  </dd>
                </>
              )}
            </dl>
          </section>
        ))}
      </section>
      <section aria-labelledby="materials-title">
        <h2 id="materials-title" lang={uiLanguage}>
          {storeReview ? "Materialen" : "Materials"}
        </h2>
        <p>{p.copy.materials}</p>
      </section>
      <section aria-labelledby="care-title">
        <h2 id="care-title" lang={uiLanguage}>
          {storeReview ? "Onderhoud" : "Care"}
        </h2>
        <p>{p.copy.care}</p>
      </section>
      <section aria-labelledby="assembly-title">
        <h2 id="assembly-title" lang={uiLanguage}>
          {storeReview ? "Opstelling" : "Assembly"}
        </h2>
        <p>{p.copy.assembly}</p>
      </section>
      <section aria-labelledby="safety-title">
        <h2 id="safety-title" lang={uiLanguage}>
          {storeReview ? "Veiligheid" : "Safety"}
        </h2>
        <p>{p.copy.safety}</p>
      </section>
      <section aria-labelledby="parcels-title" lang={uiLanguage}>
        <h2 id="parcels-title">
          {storeReview
            ? "Afmetingen en gewicht per pakket"
            : "Measured parcels"}
        </h2>
        <ol>
          {p.parcels.map((parcel, index) => (
            <li key={index}>
              <bdi>{dimensions(parcel.dimensions)}</bdi>;{" "}
              <bdi>{parcel.gross_weight_g} g</bdi>
            </li>
          ))}
        </ol>
      </section>
      <section lang={uiLanguage}>
        <h2>
          {storeReview ? "Bezorging en retourneren" : "Delivery and returns"}
        </h2>
        {storeReview ? (
          <p>
            <Link
              className="underline"
              href={`${storeInformationPath}#bezorging`}
            >
              Bezorging
            </Link>
            {" · "}
            <Link
              className="underline"
              href={`${storeInformationPath}#retourneren`}
            >
              Retourneren
            </Link>
          </p>
        ) : (
          <p>
            This private preview does not take orders. Delivery countries,
            carrier services, taxes and returns await verified market and legal
            terms.
          </p>
        )}
      </section>
    </div>
  );
}
