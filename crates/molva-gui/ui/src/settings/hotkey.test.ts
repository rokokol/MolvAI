// SPDX-License-Identifier: MIT
// Capture of a key combination: the name comes from the physical key, and a modifier alone is
// not a shortcut

import { describe, expect, it } from "vitest";

import { captureHotkey, keyName, normalizeHotkey, type KeyCapture } from "./hotkey";

function key(code: string, modifiers: Partial<KeyCapture> = {}): KeyCapture {
  return { code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, ...modifiers };
}

describe("keyName", () => {
  it("names letters, digits and function keys by their code", () => {
    expect(keyName("KeyA")).toBe("a");
    expect(keyName("Digit1")).toBe("1");
    expect(keyName("F9")).toBe("f9");
    expect(keyName("F12")).toBe("f12");
  });

  it("names the keys that have no character", () => {
    expect(keyName("Space")).toBe("space");
    expect(keyName("Escape")).toBe("escape");
    expect(keyName("ArrowLeft")).toBe("left");
    expect(keyName("Backquote")).toBe("grave");
  });

  it("has no name for a key outside the parser table and for F13", () => {
    expect(keyName("F13")).toBeNull();
    expect(keyName("NumpadAdd")).toBeNull();
    expect(keyName("IntlBackslash")).toBeNull();
  });
});

describe("captureHotkey", () => {
  it("builds the combination in the parser order", () => {
    const result = captureHotkey(key("KeyX", { ctrlKey: true, altKey: true }));
    expect(result).toEqual({ kind: "candidate", value: "Ctrl+Alt+X" });
  });

  it("puts every modifier in one order whatever the press order was", () => {
    const a = captureHotkey(key("Space", { shiftKey: true, ctrlKey: true }));
    const b = captureHotkey(key("Space", { ctrlKey: true, shiftKey: true }));
    expect(a).toEqual(b);
    expect(a).toEqual({ kind: "candidate", value: "Ctrl+Shift+Space" });
  });

  it("takes a lone function key", () => {
    expect(captureHotkey(key("F9"))).toEqual({ kind: "candidate", value: "F9" });
  });

  it("does not accept a modifier on its own", () => {
    for (const code of ["ControlLeft", "ShiftRight", "AltLeft", "MetaRight"]) {
      expect(captureHotkey(key(code, { ctrlKey: true }))).toEqual({ kind: "modifier" });
    }
  });

  it("refuses a key that the daemon cannot bind", () => {
    expect(captureHotkey(key("NumpadAdd"))).toEqual({ kind: "unsupported", code: "NumpadAdd" });
  });
});

describe("normalizeHotkey", () => {
  it("shows a stored value in the same spelling as a capture", () => {
    expect(normalizeHotkey("ctrl+alt+z")).toBe("Ctrl+Alt+Z");
    expect(normalizeHotkey("  control - shift - space ")).toBe("Ctrl+Shift+Space");
    expect(normalizeHotkey("meta+F9")).toBe("Super+F9");
  });

  it("keeps a sided modifier as the only key", () => {
    expect(normalizeHotkey("rightctrl")).toBe("RightCtrl");
  });

  it("never hides an unknown part", () => {
    expect(normalizeHotkey("Ctrl+Telepathy")).toBe("Ctrl+Telepathy");
  });

  it("gives an empty text for an empty value", () => {
    expect(normalizeHotkey("   ")).toBe("");
  });
});
