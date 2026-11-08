// SPDX-License-Identifier: MIT
// What a key press means in the path input.
//
// The decision is a pure function, so the keyboard of the autocomplete is tested without a
// browser: Arrow keys move the highlight, Tab and Enter fill the field, Enter with nothing
// highlighted starts the transcription, Escape closes the list.

export type ComboboxAction =
  | { type: "next" }
  | { type: "previous" }
  | { type: "fill"; index: number }
  | { type: "start" }
  | { type: "close" }
  | { type: "none" };

/** The index that a move of `delta` reaches, wrapping around the list */
export function moveActive(current: number, length: number, delta: number): number {
  if (length <= 0) {
    return -1;
  }
  if (current < 0) {
    return delta > 0 ? 0 : length - 1;
  }
  return (current + delta + length) % length;
}

export interface ComboboxState {
  /** How many entries the list holds */
  count: number;
  /** The highlighted entry, `-1` for none */
  active: number;
  /** Whether the list is on screen */
  visible: boolean;
}

/**
 * The action of a key
 *
 * Tab fills the highlighted entry, or the first one when nothing is highlighted, because the
 * first entry is what the list offers. Enter without a highlighted entry is the start of the
 * transcription, the same as the button.
 */
export function comboboxAction(key: string, state: ComboboxState): ComboboxAction {
  const { count, active, visible } = state;
  switch (key) {
    case "ArrowDown":
      return count > 0 ? { type: "next" } : { type: "none" };
    case "ArrowUp":
      return count > 0 ? { type: "previous" } : { type: "none" };
    case "Escape":
      return visible ? { type: "close" } : { type: "none" };
    case "Tab": {
      if (!visible || count === 0) {
        return { type: "none" };
      }
      return { type: "fill", index: active >= 0 ? active : 0 };
    }
    case "Enter": {
      if (visible && active >= 0 && active < count) {
        return { type: "fill", index: active };
      }
      return { type: "start" };
    }
    default:
      return { type: "none" };
  }
}
