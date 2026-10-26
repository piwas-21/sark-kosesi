"use client";
import { useEffect, useState } from "react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import {
  direction,
  isLocale,
  languageNames,
  locales,
  switchLocale,
  type Locale,
} from "../lib/locale";
import { isTheme, resolveTheme, type Theme } from "../lib/theme";
export function Preferences({ locale }: { locale: Locale }) {
  const [theme, setTheme] = useState<Theme | undefined>(undefined);
  const router = useRouter();
  const path = usePathname();
  const search = useSearchParams();
  useEffect(() => {
    let stored: unknown;
    try {
      stored = localStorage.getItem("theme");
    } catch {
      stored = "system";
    }
    setTheme(isTheme(stored) ? stored : "system");
  }, []);
  useEffect(() => {
    if (theme === undefined) return;
    const media = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme = resolveTheme(
        theme,
        media.matches,
      );
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
  return (
    <div className="preferences">
      <label>
        Language{" "}
        <select
          aria-label="Language"
          value={locale}
          onChange={(event) => {
            const next = event.target.value;
            if (!isLocale(next)) return;
            document.cookie = `locale=${next}; Path=/; SameSite=Lax; Max-Age=31536000`;
            router.push(
              switchLocale(
                path,
                search.size ? `?${search.toString()}` : "",
                next,
              ),
            );
          }}
        >
          {locales.map((value) => (
            <option
              value={value}
              key={value}
              lang={value}
              dir={direction(value)}
            >
              {languageNames[value]}
            </option>
          ))}
        </select>
      </label>
      <label>
        Theme{" "}
        <select
          aria-label="Theme"
          value={theme ?? "system"}
          onChange={(event) => {
            const value = event.target.value;
            if (!isTheme(value)) return;
            setTheme(value);
            try {
              localStorage.setItem("theme", value);
            } catch {
              /* Explicit choice still works for this page. */
            }
          }}
        >
          <option value="system">System</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </label>
    </div>
  );
}
