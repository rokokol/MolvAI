// SPDX-License-Identifier: MIT
// Interface translations. A key missing in the dictionary comes back as it is, so a missing
// string shows at once instead of as an empty label. The tray and the system notices read the
// same files on the Rust side, which also resolves the language: see `api.uiLanguage`

import { createContext, useContext } from "react";

import en from "./en.json";
import ru from "./ru.json";

export type Lang = "ru" | "en";

const DICTIONARIES: Record<Lang, Record<string, string>> = { ru, en };

// Each language is named in itself, so a user finds their own in a list they cannot read
export const LANGUAGES: { id: Lang; name: string }[] = [
  { id: "ru", name: "Русский" },
  { id: "en", name: "English" },
];

export function isLang(value: string): value is Lang {
  return value === "ru" || value === "en";
}

/** The `ui_language` value that takes the language from the system */
export const AUTO_LANG = "auto";

/**
 * The string for `key` with each `{name}` replaced from `vars`; a missing variable stays in
 * the text. A key missing in the language falls back to English, then to the key itself
 */
export function translate(
  lang: Lang,
  key: string,
  vars?: Record<string, string | number>,
): string {
  const text = DICTIONARIES[lang][key] ?? DICTIONARIES.en[key] ?? key;
  if (!vars) {
    return text;
  }
  return text.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

export type Translate = (key: string, vars?: Record<string, string | number>) => string;

export const I18nContext = createContext<{ lang: Lang; t: Translate }>({
  lang: "en",
  t: (key) => translate("en", key),
});

export function useI18n() {
  return useContext(I18nContext);
}
