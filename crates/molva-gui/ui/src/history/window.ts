// SPDX-License-Identifier: MIT
// Which rows of a long list are on the screen, and how much space the others take
//
// The list of phrases holds thousands of rows of different heights: one phrase is a line, the
// next is a paragraph. The window is therefore computed from measured heights where the list
// has measured them and from one estimate where it has not, and the rows outside the window
// become two pieces of padding instead of DOM nodes

/** The rows to paint, and the padding that stands for the rest */
export interface RowsWindow {
  /** First painted row */
  start: number;
  /** One past the last painted row */
  end: number;
  /** Height of the rows above the window */
  top: number;
  /** Height of the rows below the window */
  bottom: number;
}

/** The height of a row that the list has not measured yet */
export const DEFAULT_ESTIMATE = 118;

/**
 * Rows for one scroll position
 *
 * `heights[i] <= 0` means "not measured yet", and the estimate stands in for it. The overscan
 * is measured in rows, so a fast scroll meets painted rows instead of blank space
 */
export function rowsWindow(
  heights: readonly number[],
  options: {
    estimate: number;
    gap: number;
    scrollTop: number;
    viewport: number;
    overscan: number;
  },
): RowsWindow {
  const { estimate, gap, scrollTop, viewport, overscan } = options;
  const count = heights.length;
  if (count === 0) {
    return { start: 0, end: 0, top: 0, bottom: 0 };
  }
  const heightAt = (index: number): number => {
    const measured = heights[index] ?? 0;
    return measured > 0 ? measured : estimate;
  };
  // The overscan is measured in rows of the estimate: a tall row above the window must not
  // drag the window up with it
  const margin = overscan * (estimate + gap);
  const from = Math.max(0, scrollTop - margin);
  const to = Math.max(0, scrollTop + viewport + margin);

  let offset = 0;
  let start = -1;
  let end = count;
  for (let index = 0; index < count; index += 1) {
    const height = heightAt(index);
    const rowEnd = offset + height;
    if (start < 0 && rowEnd > from) {
      start = index;
    }
    if (offset >= to) {
      end = index;
      break;
    }
    offset = rowEnd + gap;
    end = index + 1;
  }
  if (start < 0) {
    start = count - 1;
  }
  if (end < start) {
    end = start;
  }

  const total = totalHeight(heights, estimate, gap);
  const above = offsetOf(heights, estimate, gap, start);
  const below = offsetOf(heights, estimate, gap, end);
  // The gap after the last painted row belongs to the padding below it, exactly as the layout
  // would have placed it; there is no such gap when the window reaches the end of the list
  const trailing = end < count ? gap : 0;
  return {
    start,
    end,
    top: above,
    bottom: Math.max(0, total - below + trailing),
  };
}

/** Height of the whole list with the estimate standing in for the unmeasured rows */
export function totalHeight(
  heights: readonly number[],
  estimate: number,
  gap: number,
): number {
  if (heights.length === 0) {
    return 0;
  }
  return offsetOf(heights, estimate, gap, heights.length) - gap;
}

/** Distance from the first row to the top of the row at `index` */
export function offsetOf(
  heights: readonly number[],
  estimate: number,
  gap: number,
  index: number,
): number {
  let offset = 0;
  for (let row = 0; row < index && row < heights.length; row += 1) {
    const measured = heights[row] ?? 0;
    offset += (measured > 0 ? measured : estimate) + gap;
  }
  return offset;
}
