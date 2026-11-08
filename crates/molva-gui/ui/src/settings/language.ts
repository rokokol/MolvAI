// SPDX-License-Identifier: MIT
// The recognition language: one field, `stt.language`, decides it
//
// `stt.allowed_languages` is not a second setting of the same thing. The daemon reads it only
// while `stt.language` is `auto`, where it holds the languages that detection may pick between;
// with a fixed language the list is not read at all. The page therefore edits the list only in
// the `auto` case and keeps the value otherwise, so a saved file stays compatible

export const AUTO_LANGUAGE = "auto";

export interface LanguageState {
  language: string;
  allowed_languages: string[];
}

/** The languages that the select offers, in the order of the list */
export const RECOGNITION_LANGUAGES = [
  "ru",
  "en",
  "de",
  "fr",
  "es",
  "it",
  "pt",
  "uk",
  "pl",
  "tr",
  "zh",
  "ja",
] as const;

/** The choice list: the fixed languages plus whatever the settings file holds */
export function languageChoices(current: string): string[] {
  const choices: string[] = [AUTO_LANGUAGE, ...RECOGNITION_LANGUAGES];
  const stored = current.trim().toLowerCase();
  if (stored !== "" && !choices.includes(stored)) {
    choices.push(stored);
  }
  return choices;
}

/** Whether detection picks among a list rather than using one language */
export function detectsAmongLanguages(language: string): boolean {
  return language.trim().toLowerCase() === AUTO_LANGUAGE;
}

/**
 * The two fields after a language change: the fixed language joins the list that detection
 * may use later, and switchi ng to `auto` keeps the list as it is
 */
export function setRecognitionLanguage(next: string, state: LanguageState): LanguageState {
  const language = next.trim().toLowerCase();
  if (detectsAmongLanguages(language)) {
    return { language: AUTO_LANGUAGE, allowed_languages: state.allowed_languages };
  }
  const allowed = state.allowed_languages.includes(language)
    ? state.allowed_languages
    : [...state.allowed_languages, language];
  return { language, allowed_languages: allowed };
}

/** A language code as a hint: `ru` → `RU`, an unknown value stays visible */
export function languageLabel(code: string): string {
  return code.length === 0 ? "" : code.toUpperCase();
}
