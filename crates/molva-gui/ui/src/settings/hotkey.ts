// SPDX-License-Identifier: MIT
// Hotkey capture: a key press becomes the text that `hotkeys.*` stores
//
// The names come from the physical key code, not from the character, so the same combination
// works in any layout. Which names exist is decided by the parser in
// `molva_core::app::hotkeys`: the page sends the candidate to the `check_hotkey` command
// before it accepts it, so this module only has to cover the keys that the parser knows

/** The part of a keyboard event that the capture reads */
export interface KeyCapture {
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** A modifier on its own is not a shortcut: the modal keeps waiting for a main key */
const MODIFIER_CODES = new Set([
  "ControlLeft",
  "ControlRight",
  "ShiftLeft",
  "ShiftRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "OSLeft",
  "OSRight",
]);

/** Named keys of the core parser, keyed by the physical code */
const NAMED_CODES: Record<string, string> = {
  Space: "space",
  Escape: "escape",
  Tab: "tab",
  Backspace: "backspace",
  Enter: "enter",
  NumpadEnter: "enter",
  Minus: "minus",
  Equal: "equal",
  Backquote: "grave",
  Backslash: "backslash",
  Home: "home",
  End: "end",
  PageUp: "pageup",
  PageDown: "pagedown",
  Insert: "insert",
  Delete: "delete",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  CapsLock: "capslock",
  NumLock: "numlock",
  ScrollLock: "scrolllock",
  PrintScreen: "printscreen",
  Pause: "pause",
  ContextMenu: "menu",
};

/** Named keys whose display form joins two words */
const DISPLAY_NAMES: Record<string, string> = {
  pageup: "PageUp",
  pagedown: "PageDown",
  numlock: "NumLock",
  scrolllock: "ScrollLock",
  printscreen: "PrintScreen",
  capslock: "CapsLock",
  backspace: "Backspace",
  leftctrl: "LeftCtrl",
  rightctrl: "RightCtrl",
  leftshift: "LeftShift",
  rightshift: "RightShift",
  leftalt: "LeftAlt",
  rightalt: "RightAlt",
  leftsuper: "LeftSuper",
  rightsuper: "RightSuper",
};

/**
 * The spelling that the page shows for a key name: a letter and a function key are upper case
 * because that is how a keyboard and the shipped defaults write them. The parser ignores case,
 * so this is presentation alone
 */
export function displayKey(name: string): string {
  if (/^[a-z]$/.test(name) || /^f([1-9]|1[0-2])$/.test(name)) {
    return name.toUpperCase();
  }
  const known = DISPLAY_NAMES[name.toLowerCase()];
  if (known) {
    return known;
  }
  return name.charAt(0).toUpperCase() + name.slice(1).toLowerCase();
}

export type CaptureResult =
  | { kind: "modifier" }
  | { kind: "unsupported"; code: string }
  | { kind: "candidate"; value: string };

/** Name of the main key, or `null` when the parser knows no such name */
export function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code);
  if (letter) {
    return letter[1].toLowerCase();
  }
  const digit = /^(?:Digit|Numpad)([0-9])$/.exec(code);
  if (digit) {
    return digit[1];
  }
  const fn = /^F([1-9]|1[0-2])$/.exec(code);
  if (fn) {
    return `f${fn[1]}`;
  }
  return NAMED_CODES[code] ?? null;
}

/**
 * The combination of one key press
 *
 * The order of the modifiers matches the parser, which sorts them: `Ctrl+Shift+Space` and
 * `Shift+Ctrl+Space` are the same binding, and one spelling keeps the field readable
 */
export function captureHotkey(event: KeyCapture): CaptureResult {
  if (MODIFIER_CODES.has(event.code)) {
    return { kind: "modifier" };
  }
  const key = keyName(event.code);
  if (key === null) {
    return { kind: "unsupported", code: event.code };
  }
  const parts: string[] = [];
  if (event.ctrlKey) {
    parts.push("Ctrl");
  }
  if (event.shiftKey) {
    parts.push("Shift");
  }
  if (event.altKey) {
    parts.push("Alt");
  }
  if (event.metaKey) {
    parts.push("Super");
  }
  parts.push(displayKey(key));
  return { kind: "candidate", value: parts.join("+") };
}

/**
 * The stored text in the form the page shows it: every part but the last is a modifier, and
 * an unknown part stays as it is, so a hand-written value is never hidden
 */
export function normalizeHotkey(text: string): string {
  const parts = text
    .split(/[+-]/)
    .map((part) => part.trim())
    .filter((part) => part !== "");
  if (parts.length === 0) {
    return "";
  }
  const modifiers = parts.slice(0, -1).map(canonicalModifier);
  const last = parts[parts.length - 1].toLowerCase();
  return [...modifiers, displayKey(last)].join("+");
}

function canonicalModifier(part: string): string {
  switch (part.toLowerCase()) {
    case "ctrl":
    case "control":
      return "Ctrl";
    case "shift":
      return "Shift";
    case "alt":
    case "option":
      return "Alt";
    case "super":
    case "meta":
    case "win":
    case "cmd":
      return "Super";
    default:
      return part;
  }
}
