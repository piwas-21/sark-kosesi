import { languageTag, type Locale } from "./locale";
export function displayPrice(minor: number, locale: Locale): string {
  if (!Number.isSafeInteger(minor) || minor < 0)
    throw new Error("Invalid public money");
  const amount = BigInt(minor);
  const cents = new Intl.NumberFormat(languageTag(locale), {
    minimumIntegerDigits: 2,
    useGrouping: false,
  }).format(amount % 100n);
  return new Intl.NumberFormat(languageTag(locale), {
    style: "currency",
    currency: "EUR",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })
    .formatToParts(amount / 100n)
    .map((part) => (part.type === "fraction" ? cents : part.value))
    .join("");
}
