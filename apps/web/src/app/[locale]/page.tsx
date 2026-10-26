import Link from "next/link";
import { notFound } from "next/navigation";
import { CourtyardMark, Status } from "@piwas/design";
import { isLocale, titles } from "../../lib/locale";
import { loadCatalogue } from "../../lib/load-catalogue";
import styles from "../../components/courtyard.module.css";
export default async function Home({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  if (!isLocale(locale)) notFound();
  const catalogue = await loadCatalogue(locale);
  return (
    <>
      <div className={styles.hero}>
        <div>
          <h1>{titles[locale]}</h1>
          <div lang="en">
            <p>Low seats. Long conversations.</p>
            <p>
              A private study of the living courtyard. Explore the application
              with clearly labelled demonstration data.
            </p>
            <a className="button" href="#collection">
              Explore the preview
            </a>
          </div>
        </div>
        <div className={styles.arch} aria-hidden="true">
          <CourtyardMark />
          <p>
            The living courtyard
            <br />A design study
          </p>
        </div>
      </div>
      <section id="collection" className={styles.collection} lang="en">
        <div>
          <h2>Collection preview</h2>
          <p>
            These invented fixtures verify the catalogue journey. Prices,
            materials and dimensions await real product evidence.
          </p>
        </div>
        <div>
          {catalogue.items.length === 0 ? (
            <Status>No demonstration items in this language.</Status>
          ) : (
            catalogue.items.map((item) => (
              <article className={styles.product} key={item.sku}>
                <p>Test fixture · Not for sale</p>
                <h3>
                  <Link href={`/${locale}/products/${item.slug}`}>
                    {item.title}
                  </Link>
                </h3>
                <p>{item.description}</p>
                <p>
                  Sample price:{" "}
                  {new Intl.NumberFormat(locale === "ku" ? "ku-Latn" : locale, {
                    style: "currency",
                    currency: "EUR",
                  }).format(item.minor_units / 100)}
                </p>
                <p>Configuration: {item.fill}</p>
              </article>
            ))
          )}
        </div>
      </section>
    </>
  );
}
