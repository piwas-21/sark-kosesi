import { notFound } from "next/navigation";
import { configuration } from "../../../lib/config";
import styles from "../../../components/store-review.module.css";

export default async function StoreInformation({
  params,
}: {
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;
  const { storeReview } = configuration();
  if (locale !== "nl" || !storeReview) notFound();

  const { sellerName, kvk, vatId, address, email, phone, policies } =
    storeReview;
  const sections = [
    { id: "bezorging", title: "Bezorging", text: policies.delivery },
    { id: "retourneren", title: "Retourneren", text: policies.returns },
    { id: "privacy", title: "Privacy", text: policies.privacy },
    { id: "betalen", title: "Betalen", text: policies.payment },
    { id: "voorwaarden", title: "Verkoopvoorwaarden", text: policies.terms },
  ];

  return (
    <div className="wrap-anywhere">
      <h1 className={styles.informationTitle}>Winkelinformatie</h1>
      <section
        aria-labelledby="contact-title"
        className="border-[var(--line)] border-b pb-8"
      >
        <h2 id="contact-title" className="mb-4 text-3xl">
          Verkoper en contact
        </h2>
        <address className="space-y-3 not-italic">
          <p className="font-bold">{sellerName}</p>
          <p className="whitespace-pre-line">{address}</p>
          <p>
            <a className="underline" href={`mailto:${email}`}>
              {email}
            </a>
          </p>
          <p>
            <a className="underline" href={`tel:${phone}`}>
              {phone}
            </a>
          </p>
        </address>
        <dl className="mt-4 grid max-w-md grid-cols-2 gap-2">
          <dt>KvK-nummer</dt>
          <dd>{kvk}</dd>
          <dt>Btw-nummer</dt>
          <dd>{vatId}</dd>
        </dl>
      </section>
      {sections.map(({ id, title, text }) => (
        <section
          key={id}
          id={id}
          aria-labelledby={`${id}-title`}
          className="border-[var(--line)] border-b pb-8"
        >
          <h2 id={`${id}-title`} className="mb-4 text-3xl">
            {title}
          </h2>
          <p className="whitespace-pre-line">{text}</p>
        </section>
      ))}
    </div>
  );
}
