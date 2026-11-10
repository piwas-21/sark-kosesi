import type { Locale } from "../../lib/locale";
import draftCopy from "./copy.json";
import fileCopy from "./files-copy.json";

export type PlannerCopy = typeof draftCopy.en & { files: typeof fileCopy.en };
export const plannerCopy: Record<Locale, PlannerCopy> = {
  nl: { ...draftCopy.nl, files: fileCopy.nl },
  de: { ...draftCopy.de, files: fileCopy.de },
  fr: { ...draftCopy.fr, files: fileCopy.fr },
  en: { ...draftCopy.en, files: fileCopy.en },
  ar: { ...draftCopy.ar, files: fileCopy.ar },
  tr: { ...draftCopy.tr, files: fileCopy.tr },
  ku: { ...draftCopy.ku, files: fileCopy.kmr },
};
