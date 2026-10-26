export const locales = ["nl", "de", "fr", "en", "ar", "tr", "ku"] as const;
export type Locale = (typeof locales)[number];
export function isLocale(value: string): value is Locale {
  return locales.some((locale) => locale === value);
}
export function languageTag(locale: Locale): string {
  return locale === "ku" ? "ku-Latn" : locale;
}
export function direction(locale: Locale): "rtl" | "ltr" {
  return locale === "ar" ? "rtl" : "ltr";
}
export function deviceLocale(tag: string): Locale | undefined {
  const normalized = tag.toLowerCase().replaceAll("_", "-");
  if (
    normalized === "kmr" ||
    normalized.startsWith("kmr-") ||
    normalized === "ku" ||
    normalized === "ku-latn" ||
    normalized.startsWith("ku-latn-")
  )
    return "ku";
  const base = normalized.split("-")[0];
  if (base && base !== "ku" && isLocale(base)) return base;
  return undefined;
}
export function resolveLocale(
  saved: string | undefined,
  accept: string,
): Locale {
  if (saved && isLocale(saved)) return saved;
  const ranked = accept
    .slice(0, 4096)
    .split(",")
    .map((part, index) => {
      const [tag = "", ...parameters] = part.trim().split(";");
      const quality = parameters.find((parameter) =>
        parameter.trim().startsWith("q="),
      );
      const value = quality ? Number(quality.trim().slice(2)) : 1;
      return {
        locale: deviceLocale(tag),
        quality: Number.isFinite(value) && value > 0 && value <= 1 ? value : 0,
        index,
      };
    })
    .sort(
      (left, right) => right.quality - left.quality || left.index - right.index,
    );
  return (
    ranked.find((entry) => entry.quality > 0 && entry.locale)?.locale ?? "nl"
  );
}
export function switchLocale(
  path: string,
  search: string,
  locale: Locale,
): string {
  const segments = path.split("/");
  if (isLocale(segments[1] ?? "")) segments[1] = locale;
  else segments.splice(1, 0, locale);
  return segments.join("/") + search;
}
export const titles: Record<Locale, string> = {
  nl: "Blijf nog even.",
  de: "Bleib noch ein wenig.",
  fr: "Restez encore un peu.",
  en: "Stay, a little.",
  ar: "ابقَ قليلًا.",
  tr: "Biraz daha kal.",
  ku: "Hinekî din bimîne.",
};
export const languageNames: Record<Locale, string> = {
  nl: "Nederlands",
  de: "Deutsch",
  fr: "Français",
  en: "English",
  ar: "العربية",
  tr: "Türkçe",
  ku: "Kurmancî",
};
