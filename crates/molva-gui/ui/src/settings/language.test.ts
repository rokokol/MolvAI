// SPDX-License-Identifier: MIT
// One field for the recognition language, and the allowed list only where the daemon reads it

import { describe, expect, it } from "vitest";

import {
  AUTO_LANGUAGE,
  detectsAmongLanguages,
  languageChoices,
  setRecognitionLanguage,
} from "./language";

describe("setRecognitionLanguage", () => {
  it("makes the fixed language the value of stt.language", () => {
    expect(setRecognitionLanguage("ru", { language: "en", allowed_languages: ["ru", "en"] })).toEqual(
      { language: "ru", allowed_languages: ["ru", "en"] },
    );
  });

  it("adds a new fixed language to the list that detection may use later", () => {
    expect(
      setRecognitionLanguage("de", { language: "ru", allowed_languages: ["ru", "en"] }),
    ).toEqual({ language: "de", allowed_languages: ["ru", "en", "de"] });
  });

  it("does not repeat a language that the list already holds", () => {
    expect(
      setRecognitionLanguage("EN", { language: "ru", allowed_languages: ["ru", "en"] }),
    ).toEqual({ language: "en", allowed_languages: ["ru", "en"] });
  });

  it("keeps the list untouched when detection returns", () => {
    expect(
      setRecognitionLanguage(AUTO_LANGUAGE, { language: "de", allowed_languages: ["ru", "en"] }),
    ).toEqual({ language: AUTO_LANGUAGE, allowed_languages: ["ru", "en"] });
  });
});

describe("detectsAmongLanguages", () => {
  it("is true for auto and for its spellings only", () => {
    expect(detectsAmongLanguages("auto")).toBe(true);
    expect(detectsAmongLanguages(" AUTO ")).toBe(true);
    expect(detectsAmongLanguages("ru")).toBe(false);
  });
});

describe("languageChoices", () => {
  it("starts with auto and offers the common languages", () => {
    const choices = languageChoices("ru");
    expect(choices[0]).toBe(AUTO_LANGUAGE);
    expect(choices).toContain("ru");
    expect(choices).toContain("en");
  });

  it("keeps a stored code that the list does not know", () => {
    expect(languageChoices("eo")).toContain("eo");
    expect(languageChoices("ru")).not.toContain("eo");
  });
});
