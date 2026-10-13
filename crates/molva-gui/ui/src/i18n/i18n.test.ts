// SPDX-License-Identifier: MIT
// Lookup rules of the window's dictionaries. Which language applies is resolved on the Rust
// side and tested there, in `crates/molva-gui/src/i18n.rs`

import { describe, expect, it } from "vitest";

import en from "./en.json";
import ru from "./ru.json";
import { translate } from "./index";

describe("translate", () => {
  it("takes the string from the dictionary of the language", () => {
    expect(translate("ru", "tray.quit")).toBe(ru["tray.quit"]);
    expect(translate("en", "tray.quit")).toBe(en["tray.quit"]);
    expect(ru["tray.quit"]).not.toBe(en["tray.quit"]);
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
