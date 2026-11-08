// SPDX-License-Identifier: MIT
// The key source decides which field the page shows, and only the key store gets a key through
// a command

import { describe, expect, it } from "vitest";

import {
  keyField,
  keySourceChoices,
  KEY_SOURCE_ORDER,
  storesInKeyring,
  storeUnavailable,
  warnsAboutClearText,
} from "./keysource";

describe("keySourceChoices", () => {
  it("offers keyring first, then the plain key and the variable", () => {
    expect(keySourceChoices("keyring")).toEqual([...KEY_SOURCE_ORDER]);
    expect(keySourceChoices("keyring")[0]).toBe("keyring");
  });

  it("keeps a stored value that the page does not offer, so no select shows nothing", () => {
    expect(keySourceChoices("file")).toContain("file");
    expect(keySourceChoices("keyring")).not.toContain("file");
  });
});

describe("keyField", () => {
  it("maps each source to the field it needs", () => {
    expect(keyField("keyring")).toBe("store");
    expect(keyField("plain")).toBe("plain");
    expect(keyField("env")).toBe("env");
    expect(keyField("none")).toBe("none");
    expect(keyField("file")).toBe("legacy");
  });

  it("sends a pasted key to the key store only for the keyring source", () => {
    expect(storesInKeyring("keyring")).toBe(true);
    for (const source of ["plain", "env", "none", "file"]) {
      expect(storesInKeyring(source)).toBe(false);
    }
  });

  it("warns about clear text only for the plain source", () => {
    expect(warnsAboutClearText("plain")).toBe(true);
    expect(warnsAboutClearText("keyring")).toBe(false);
    expect(warnsAboutClearText("env")).toBe(false);
  });
});

describe("storeUnavailable", () => {
  it("tells a refusal of the store from a missing entry", () => {
    expect(storeUnavailable({})).toBe(false);
    expect(storeUnavailable({ detail: "key store not available: no bus" })).toBe(true);
  });
});
