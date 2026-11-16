// SPDX-License-Identifier: MIT
// The window over the journal: which rows are painted, and how much space the others take

import { describe, expect, it } from "vitest";

import { DEFAULT_ESTIMATE, offsetOf, rowsWindow, totalHeight } from "./window";

const options = {
  estimate: 100,
  gap: 10,
  viewport: 300,
  overscan: 1,
};

describe("the window of a list", () => {
  it("paints nothing for an empty list", () => {
    expect(rowsWindow([], { ...options, scrollTop: 0 })).toEqual({
      start: 0,
      end: 0,
      top: 0,
      bottom: 0,
    });
  });

  it("paints the top of the list at the top of the page", () => {
    const heights = new Array(20).fill(0);
    const window = rowsWindow(heights, { ...options, scrollTop: 0 });
    expect(window.start).toBe(0);
    expect(window.top).toBe(0);
    // One row of overscan below a viewport of three rows of 110 px
    expect(window.end).toBeGreaterThanOrEqual(4);
    expect(window.end).toBeLessThan(20);
  });

  it("moves the window with the scroll and keeps the total height", () => {
    const heights = new Array(100).fill(0);
    const window = rowsWindow(heights, { ...options, scrollTop: 2_200 });
    expect(window.start).toBeGreaterThan(10);
    expect(window.end).toBeGreaterThan(window.start);
    // Padding plus painted rows is the whole list, whatever the scroll position
    const total = totalHeight(heights, options.estimate, options.gap);
    const painted =
      window.end > window.start
        ? offsetOf(heights, options.estimate, options.gap, window.end) -
          offsetOf(heights, options.estimate, options.gap, window.start) -
          options.gap
        : 0;
    expect(window.top + painted + window.bottom).toBeCloseTo(total, 5);
  });

  it("takes a measured height instead of the estimate", () => {
    const heights = new Array(50).fill(0);
    heights[0] = 400;
    const window = rowsWindow(heights, { ...options, scrollTop: 0 });
    // The tall first row already fills the viewport
    expect(window.start).toBe(0);
    expect(window.end).toBeLessThanOrEqual(4);

    const scrolled = rowsWindow(heights, { ...options, scrollTop: 1_000 });
    expect(scrolled.start).toBeGreaterThanOrEqual(1);
    expect(scrolled.top).toBeGreaterThanOrEqual(400);
  });

  it("keeps the last row reachable at the end of the list", () => {
    const heights = new Array(30).fill(0);
    const total = totalHeight(heights, options.estimate, options.gap);
    const window = rowsWindow(heights, {
      ...options,
      scrollTop: total - options.viewport,
    });
    expect(window.end).toBe(30);
    expect(window.bottom).toBe(0);

    const beyond = rowsWindow(heights, { ...options, scrollTop: total });
    expect(beyond.start).toBeGreaterThan(0);
    expect(beyond.end).toBe(30);
  });

  it("counts the gap between rows once", () => {
    expect(totalHeight([], 100, 10)).toBe(0);
    expect(totalHeight([100], 100, 10)).toBe(100);
    expect(totalHeight([100, 100], 100, 10)).toBe(210);
    expect(totalHeight([0, 0], DEFAULT_ESTIMATE, 10)).toBe(DEFAULT_ESTIMATE * 2 + 10);
    expect(offsetOf([100, 100], 100, 10, 1)).toBe(110);
  });
});
