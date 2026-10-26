import { notFound } from "next/navigation";
import { isLocale } from "../../../../lib/locale";
import { loadCatalogue } from "../../../../lib/load-catalogue";
import styles from "../../../../components/courtyard.module.css";
export default async function ProductPage({
  params,
}: {
  params: Promise<{ locale: string; slug: string }>;
}) {
  const { locale, slug } = await params;
  if (!isLocale(locale)) notFound();
  const item = (await loadCatalogue(locale)).items.find(
    (item) => item.slug === slug,
  );
  if (!item) notFound();
  return (
    <article className={styles.product} lang="en">
      <p>Test fixture · Not for sale</p>
      <h1>{item.title}</h1>
      <p>{item.description}</p>
      <dl>
        <dt>Fixture SKU</dt>
        <dd>
          <bdi>{item.sku}</bdi>
        </dd>
        <dt>Sample price</dt>
        <dd>EUR {item.minor_units / 100}</dd>
        <dt>Configuration</dt>
        <dd>{item.fill}</dd>
        <dt>Care</dt>
        <dd>{item.care}</dd>
      </dl>
      <p>
        Real measurements, materials, photographs, delivery and returns remain
        pending. Checkout is disabled.
      </p>
    </article>
  );
}
