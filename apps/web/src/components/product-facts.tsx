import type { PublishedProduct } from "../lib/published-catalogue";
import styles from "./published.module.css";
function dimensions(d: PublishedProduct["footprint"]) {
  return `${d.width_mm} × ${d.depth_mm} × ${d.height_mm} mm (± ${d.tolerance_mm} mm)`;
}
export function ProductFacts({ product: p }: { product: PublishedProduct }) {
  return (
    <div className={styles.facts}>
      <section aria-labelledby="included-title">
        <h2 id="included-title" lang="en">
          What is included
        </h2>
        <dl lang="en">
          <dt>SKU</dt>
          <dd>
            <bdi>{p.sku}</bdi>
          </dd>
          <dt>Configuration</dt>
          <dd>{p.fill}</dd>
          <dt>Size</dt>
          <dd>{p.size_code}</dd>
          <dt>Textile</dt>
          <dd>{p.textile_code}</dd>
          <dt>Assembled footprint</dt>
          <dd>
            <bdi>{dimensions(p.footprint)}</bdi>
          </dd>
        </dl>
        {p.components.map((c) => (
          <section key={c.code} lang="en">
            <h3>
              <bdi>{c.code}</bdi>
            </h3>
            <dl>
              <dt>Quantity</dt>
              <dd>{c.quantity}</dd>
              <dt>Measurements</dt>
              <dd>
                <bdi>{dimensions(c.dimensions)}</bdi>
              </dd>
              {c.material.kind === "cover" ? (
                <>
                  <dt>Fibre composition</dt>
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
                  <dt>Foam density</dt>
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
        <h2 id="materials-title" lang="en">
          Materials
        </h2>
        <p>{p.copy.materials}</p>
      </section>
      <section aria-labelledby="care-title">
        <h2 id="care-title" lang="en">
          Care
        </h2>
        <p>{p.copy.care}</p>
      </section>
      <section aria-labelledby="assembly-title">
        <h2 id="assembly-title" lang="en">
          Assembly
        </h2>
        <p>{p.copy.assembly}</p>
      </section>
      <section aria-labelledby="safety-title">
        <h2 id="safety-title" lang="en">
          Safety
        </h2>
        <p>{p.copy.safety}</p>
      </section>
      <section aria-labelledby="parcels-title" lang="en">
        <h2 id="parcels-title">Measured parcels</h2>
        <ol>
          {p.parcels.map((parcel, index) => (
            <li key={index}>
              <bdi>{dimensions(parcel.dimensions)}</bdi>;{" "}
              <bdi>{parcel.gross_weight_g} g</bdi>
            </li>
          ))}
        </ol>
      </section>
      <section lang="en">
        <h2>Delivery and returns</h2>
        <p>
          This private preview does not take orders. Delivery countries, carrier
          services, taxes and returns await verified market and legal terms.
        </p>
      </section>
    </div>
  );
}
