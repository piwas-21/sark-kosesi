import type { Metadata } from "next";
import Link from "next/link";
import { notFound, redirect } from "next/navigation";
import { isLocale } from "../../../../lib/locale";
import { configuration } from "../../../../lib/config";
import { publishedCollection } from "../../../../lib/load-published";
import {
  slug as validSlug,
  merchantCode,
} from "../../../../lib/published-validation";
import {
  displayPrice,
  imagePath,
  productPath,
} from "../../../../lib/published-presentation";
import { ProductFacts } from "../../../../components/product-facts";
import styles from "../../../../components/published.module.css";
type Props = {
  params: Promise<{ locale: string; slug: string }>;
  searchParams: Promise<{ variant?: string }>;
};
export async function generateMetadata({ params }: Props): Promise<Metadata> {
  const { locale, slug } = await params;
  if (!isLocale(locale) || !validSlug(slug)) return {};
  return {
    alternates: {
      canonical: `${configuration().canonical}/${locale}/collection/${slug}`,
    },
    robots: { index: false, follow: false },
  };
}
export default async function Family({ params, searchParams }: Props) {
  const { locale, slug } = await params;
  const { variant } = await searchParams;
  if (
    !isLocale(locale) ||
    !validSlug(slug) ||
    (variant !== undefined && !merchantCode(variant))
  )
    notFound();
  // An explicitly requested SKU either resolves exactly or fails; it never selects a sibling.
  const products = await publishedCollection(locale);
  const selected =
    variant !== undefined
      ? products.find((p) => p.sku === variant)
      : products.find((p) => p.family_slug === slug);
  if (!selected) notFound();
  if (selected.family_slug !== slug) redirect(productPath(selected));
  const variants = products.filter(
    (p) => p.family_code === selected.family_code,
  );
  const image = selected.images[0];
  return (
    <article data-selected-sku={selected.sku}>
      <p lang="en">
        <Link href={`/${locale}/collection`}>Collection</Link> /{" "}
        <bdi>{selected.family_code}</bdi>
      </p>
      <div className={styles.family}>
        <figure>
          {
            <img
              src={imagePath(image.url)}
              width={image.width}
              height={image.height}
              alt={selected.copy.title}
            />
          }
          <figcaption lang="en">
            Selected configuration: <bdi>{selected.sku}</bdi>
          </figcaption>
        </figure>
        <div>
          <p lang="en">Private preview · Not for sale</p>
          <h1>{selected.copy.title}</h1>
          <p>{selected.copy.description}</p>
          <p className={styles.price} data-price={selected.price_decimal}>
            {displayPrice(selected.minor_units, locale)}
          </p>
          <p lang="en" data-availability={selected.availability}>
            {selected.availability === "in_stock"
              ? "Available"
              : "Out of stock"}
          </p>
          <nav aria-label="Configuration" lang="en">
            <ul className={styles.variants}>
              {variants.map((p) => (
                <li key={p.sku}>
                  <Link
                    href={productPath(p)}
                    aria-current={p.sku === selected.sku ? "true" : undefined}
                  >
                    {p.fill} · {p.size_code} · <bdi>{p.sku}</bdi>
                  </Link>
                </li>
              ))}
            </ul>
          </nav>
          <p lang="en">
            Choose a configuration to see its exact contents, price and physical
            stock availability. Checkout is disabled.
          </p>
        </div>
      </div>
      <ProductFacts product={selected} />
    </article>
  );
}
