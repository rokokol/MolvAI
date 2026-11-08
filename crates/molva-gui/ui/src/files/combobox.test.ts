// SPDX-License-Identifier: MIT
// Tests of the keyboard of the path autocomplete, without a browser

import { describe, expect, it } from "vitest";

import { comboboxAction, moveActive } from "./combobox";

const list = { count: 3, active: -1, visible: true };

describe("moveActive", () => {
  it("starts at either end of the list", () => {
    expect(moveActive(-1, 3, 1)).toBe(0);
    expect(moveActive(-1, 3, -1)).toBe(2);
  });

  it("wraps around", () => {
    expect(moveActive(2, 3, 1)).toBe(0);
    expect(moveActive(0, 3, -1)).toBe(2);
  });

  it("has no position in an empty list", () => {
    expect(moveActive(-1, 0, 1)).toBe(-1);
  });
});

describe("comboboxAction", () => {
  it("moves the highlight with the arrow keys and asks the field to prevent the cursor jump", () => {
    expect(comboboxAction("ArrowDown", list)).toEqual({ type: "next" });
    expect(comboboxAction("ArrowUp", list)).toEqual({ type: "previous" });
    expect(comboboxAction("ArrowDown", { ...list, count: 0 })).toEqual({ type: "none" });
  });

  it("fills the highlighted entry on Enter instead of starting", () => {
    expect(comboboxAction("Enter", { ...list, active: 1 })).toEqual({
      type: "fill",
      index: 1,
    });
  });

  it("starts on Enter when nothing is highlighted", () => {
    expect(comboboxAction("Enter", list)).toEqual({ type: "start" });
    expect(comboboxAction("Enter", { ...list, visible: false })).toEqual({
      type: "start",
    });
  });

  it("fills the first entry on Tab and leaves Tab alone without a list", () => {
    expect(comboboxAction("Tab", list)).toEqual({ type: "fill", index: 0 });
    expect(comboboxAction("Tab", { ...list, active: 2 })).toEqual({
      type: "fill",
      index: 2,
    });
    expect(comboboxAction("Tab", { ...list, visible: false })).toEqual({ type: "none" });
  });

  it("closes the list on Escape and only when it is open", () => {
    expect(comboboxAction("Escape", list)).toEqual({ type: "close" });
    expect(comboboxAction("Escape", { ...list, visible: false })).toEqual({
      type: "none",
    });
  });

  it("leaves ordinary typing to the field", () => {
    expect(comboboxAction("a", list)).toEqual({ type: "none" });
    expect(comboboxAction("Backspace", list)).toEqual({ type: "none" });
  });
});
