import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { en, type MessageKey } from "./en";
import { tr } from "./tr";

export type Lang = "en" | "tr";
export const LANGUAGES: { id: Lang; label: string }[] = [
  { id: "en", label: "English" },
  { id: "tr", label: "Türkçe" },
];
const DICTS: Record<Lang, Record<MessageKey, string>> = { en, tr };
const KEY = "bozkurt.lang";

export type TFn = (key: MessageKey, vars?: Record<string, string | number | undefined>) => string;

export function detectLanguage(): Lang {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved === "en" || saved === "tr") return saved;
  } catch {
    /* no storage */
  }
  const nav = typeof navigator !== "undefined" ? navigator.languages?.[0] ?? navigator.language : "en";
  return nav?.toLowerCase().startsWith("tr") ? "tr" : "en";
}

export function translate(lang: Lang, key: MessageKey, vars?: Record<string, string | number | undefined>): string {
  const s = DICTS[lang][key] ?? en[key] ?? key;
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (m, k: string) => (vars[k] === undefined ? m : String(vars[k])));
}

interface I18nCtx {
  lang: Lang;
  setLang: (l: Lang) => void;
  t: TFn;
  fmtDate: (iso: string) => string;
  fmtRelative: (iso: string) => string;
}

const Ctx = createContext<I18nCtx | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLangState] = useState<Lang>(detectLanguage);
  const setLang = useCallback((l: Lang) => {
    localStorage.setItem(KEY, l);
    setLangState(l);
  }, []);
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);
  const value = useMemo<I18nCtx>(() => {
    const locale = lang === "tr" ? "tr-TR" : "en-US";
    const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
    return {
      lang,
      setLang,
      t: (k, v) => translate(lang, k, v),
      fmtDate: (iso) => {
        const d = new Date(iso);
        return isNaN(d.getTime()) ? iso : d.toLocaleDateString(locale, { year: "numeric", month: "short", day: "numeric" });
      },
      fmtRelative: (iso) => {
        const diff = (new Date(iso).getTime() - Date.now()) / 1000;
        const abs = Math.abs(diff);
        if (abs < 60) return rtf.format(Math.round(diff), "second");
        if (abs < 3600) return rtf.format(Math.round(diff / 60), "minute");
        if (abs < 86400) return rtf.format(Math.round(diff / 3600), "hour");
        return rtf.format(Math.round(diff / 86400), "day");
      },
    };
  }, [lang, setLang]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18nCtx {
  const c = useContext(Ctx);
  if (!c) throw new Error("useI18n outside I18nProvider");
  return c;
}
export const useT = () => useI18n().t;
export type { MessageKey };
