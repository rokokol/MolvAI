// SPDX-License-Identifier: MIT
// Lookup rules of the window's dictionaries. Which language applies is resolved on the Rust
// side and tested there, in `crates/molva-gui/src/i18n.rs`

import { describe, expect, it } from "vitest";

import en from "./en.json";
import enRaw from "./en.json?raw";
import ru from "./ru.json";
import ruRaw from "./ru.json?raw";
import { translate } from "./index";

describe("translate", () => {
  it("takes the string from the dictionary of the language", () => {
    expect(translate("ru", "tray.quit")).toBe(ru["tray.quit"]);
    expect(translate("en", "tray.quit")).toBe(en["tray.quit"]);
    expect(ru["tray.quit"]).not.toBe(en["tray.quit"]);
  });

  it("has the same keys in both languages and no empty text", () => {
    const english: Record<string, string> = en;
    const russian: Record<string, string> = ru;
    expect(Object.keys(russian).sort()).toEqual(Object.keys(english).sort());
    for (const [key, text] of Object.entries(english)) {
      expect(text.trim(), key).not.toBe("");
    }
    for (const [key, text] of Object.entries(russian)) {
      expect(text.trim(), key).not.toBe("");
    }
  });

  it("keeps every placeholder of the English text in the Russian one", () => {
    const english: Record<string, string> = en;
    const russian: Record<string, string> = ru;
    const placeholders = (text: string) =>
      [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
    for (const [key, text] of Object.entries(english)) {
      expect(placeholders(russian[key] ?? ""), key).toEqual(placeholders(text));
    }
  });

  it("declares each key once, so a second copy cannot silently win", () => {
    // JSON.parse keeps the last of two equal keys, so a copy added below an existing text
    // overrides it and no other test notices
    for (const [name, raw] of [["en", enRaw], ["ru", ruRaw]] as const) {
      const keys = [...raw.matchAll(/^\s*"([^"]+)"\s*:/gm)].map((match) => match[1]);
      const seen = new Set<string>();
      const twice = keys.filter((key) => (seen.has(key) ? true : (seen.add(key), false)));
      expect(twice, name).toEqual([]);
    }
  });

  it("has no key left from the removed Search tab", () => {
    const english: Record<string, string> = en;
    const russian: Record<string, string> = ru;
    expect(english["nav.search"]).toBeUndefined();
    expect(russian["nav.search"]).toBeUndefined();
  });

  it("shows a missing key as it is", () => {
    expect(translate("ru", "no.such.key")).toBe("no.such.key");
  });

  it("substitutes variables and leaves a missing one visible", () => {
    const text = translate("en", "validation.range", { min: 1 });
    expect(text).toContain("1");
    expect(text).toContain("{max}");
  });
});
