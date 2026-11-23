import Link from "next/link";
import { notFound } from "next/navigation";
import { configuration } from "../../../lib/config";
import { storeReviewFill } from "../../../lib/store-review";
import { isLocale } from "../../../lib/locale";
import { publishedPage } from "../../../lib/load-published";
import { merchantCode } from "../../../lib/published-validation";
import {
  displayPrice,
  imagePath,
  productPath,
} from "../../../lib/published-presentation";
import styles from "../../../components/published.module.css";
export default async function Collection({
  params,
  searchParams,
}: {
  params: Promise<{ locale: string }>;
  searchParams: Promise<{ after?: string }>;
}) {
  const { locale } = await params;
  const { after } = await searchParams;
  if (!isLocale(locale) || (after !== undefined && !merchantCode(after)))
    notFound();
  const { storeReview } = configuration();
  const page = await publishedPage(locale, after);
  if (storeReview && !page.items.length) notFound();
  return (
    <>
      {storeReview ? (
        <div className={styles.intro}>
          <h1>De collectie</h1>
          <p>
            Bekijk de uitvoeringen en productdetails. Online bestellen is nog
            niet beschikbaar.
          </p>
        </div>
      ) : (
        <div className={styles.intro} lang="en">
          <p>Private collection preview · Not for sale</p>
          <h1>A place to gather.</h1>
          <p>
            Explore the available configurations and their reviewed product
            details.
          </p>
        </div>
      )}
      <div className={styles.list}>
        {page.items.map((p) => (
          <article className={styles.entry} key={p.sku}>
            {
              <img
                src={imagePath(p.images[0].url)}
                width={p.images[0].width}
                height={p.images[0].height}
                alt={p.copy.title}
              />
            }
            <h2>
              <Link href={productPath(p)}>{p.copy.title}</Link>
            </h2>
            <p>{p.copy.description}</p>
            <p className={styles.price}>
              {displayPrice(p.minor_units, locale)}
              {storeReview && <span className="ms-2 text-sm">incl. btw</span>}
            </p>
            {storeReview ? (
              <p>{storeReviewFill(p.fill)}</p>
            ) : (
              <p lang="en">
                {p.fill} ·{" "}
                {p.availability === "in_stock" ? "Available" : "Out of stock"}
              </p>
            )}
            <p>
              <bdi>{p.sku}</bdi>
            </p>
          </article>
        ))}
      </div>
      {page.items.length === 0 && (
        <p lang="en">
          No product details are available in this language yet. Choose another
          language or return later.
        </p>
      )}
      {page.next_after && (
        <Link
          className={styles.next}
          href={`/${locale}/collection?after=${page.next_after}`}
          lang={storeReview ? "nl" : "en"}
        >
          {storeReview
            ? "Meer uit de collectie"
            : "Continue through the collection"}
        </Link>
      )}
      {!storeReview && (
        <p lang="en">
          <Link href={`/${locale}/catalogue.xml`}>Preview catalogue feed</Link>
        </p>
      )}
    </>
  );
}
