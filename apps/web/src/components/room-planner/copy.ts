import type { Locale } from "../../lib/locale";
import draftCopy from "./copy.json";

export type PlannerCopy = typeof draftCopy.en;
export const plannerCopy: Record<Locale, PlannerCopy> = draftCopy;
