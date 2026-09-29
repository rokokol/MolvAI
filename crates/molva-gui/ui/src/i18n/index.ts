// SPDX-License-Identifier: MIT
// Переводы интерфейса. Ключ, которого нет в словаре, возвращается как есть —
// так пропущенная строка видна сразу, а не подменяется пустотой.

import { createContext, useContext } from "react";

import en from "./en.json";
import ru from "./ru.json";

export type Lang = "ru" | "en";

const DICTIONARIES: Record<Lang, Record<string, string>> = { ru, en };

export const LANGUAGES: { id: Lang; name: string }[] = [
  { id: "ru", name: "Русский" },
  { id: "en", name: "English" },
];

export function isLang(value: string): value is Lang {
  return value === "ru" || value === "en";
}

/** Значение `ui_language`, при котором язык берётся из системы. */
export const AUTO_LANG = "auto";

/**
 * Язык интерфейса: явная настройка, иначе язык системы.
 * `systemLanguage` — это `navigator.language`, его webview берёт из системы.
 */
export function resolveLang(configured: string | undefined, systemLanguage: string): Lang {
  if (configured !== undefined && isLang(configured)) {
    return configured;
  }
  // Английский — запасной: каталоги и сборочные машины запускают приложение с локалью C
  const primary = systemLanguage.toLowerCase().split(/[-_]/)[0];
  return isLang(primary) ? primary : "en";
}

/** Подстановка `{name}` из `vars`; отсутствующая переменная остаётся в тексте. */
export function translate(
  lang: Lang,
  key: string,
  vars?: Record<string, string | number>,
): string {
  const text = DICTIONARIES[lang][key] ?? DICTIONARIES.ru[key] ?? key;
  if (!vars) {
    return text;
  }
  return text.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in vars ? String(vars[name]) : match,
  );
}

export type Translate = (key: string, vars?: Record<string, string | number>) => string;

export const I18nContext = createContext<{ lang: Lang; t: Translate }>({
  lang: "ru",
  t: (key) => translate("ru", key),
});

export function useI18n() {
  return useContext(I18nContext);
}
