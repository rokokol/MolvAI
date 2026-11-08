// SPDX-License-Identifier: MIT
// The dashboard's live signal: one pixel column per level event, on the same lattice, with the
// same four tones and the same ordered dithering the kit's spectrogram uses.
//
// The arithmetic lives here, apart from the canvas, because that is the part a test can hold:
// how a level from the daemon becomes a column, and how a column becomes lit cells

/** Columns the window remembers: more than any window shows, at 200 % zoom too */
export const SIGNAL_HISTORY = 160;

/** A cell of the lattice, in CSS pixels, as the kit draws it */
export const CELL = 8;

/** The gap that keeps neighbouring cells apart */
export const CELL_GAP = 2;

/** The 8×8 Bayer matrix of the kit, mapped to thresholds in the open interval (0, 1) */
const BAYER_ORDER = [
  [0, 32, 8, 40, 2, 34, 10, 42],
  [48, 16, 56, 24, 50, 18, 58, 26],
  [12, 44, 4, 36, 14, 46, 6, 38],
  [60, 28, 52, 20, 62, 30, 54, 22],
  [3, 35, 11, 43, 1, 33, 9, 41],
  [51, 19, 59, 27, 49, 17, 57, 25],
  [15, 47, 7, 39, 13, 45, 5, 37],
  [63, 31, 55, 23, 61, 29, 53, 21],
];

const BAYER = BAYER_ORDER.map((row) => row.map((value) => (value + 0.5) / 64));

function clamp01(value: number): number {
  if (!Number.isFinite(value) || value <= 0) {
    return 0;
  }
  return value >= 1 ? 1 : value;
}

/**
 * A level from the daemon as the height of a column.
 *
 * The daemon reports RMS, and speech spends most of its time well below one: the square root
 * lifts the quiet half of the range so a normal phrase fills half a column instead of a
 * sliver. The gain stops the loudest phrase at the top instead of clipping it below
 */
export function signalValue(level: number): number {
  if (!Number.isFinite(level) || level <= 0) {
    return 0;
  }
  return Math.min(1, Math.sqrt(level) * 1.4);
}

/** The same value as the percentage the readout shows */
export function signalPercent(level: number): number {
  return Math.round(signalValue(level) * 100);
}

/** How many cells of a column a value lights, from the bottom up */
export function litRows(value: number, rows: number): number {
  if (rows <= 0) {
    return 0;
  }
  const level = clamp01(value);
  if (level === 0) {
    return 0;
  }
  return Math.min(rows, Math.max(1, Math.round(level * rows)));
}

/** Add a value to the history and keep the newest `limit` of them */
export function pushLevel(
  history: number[],
  value: number,
  limit: number = SIGNAL_HISTORY,
): void {
  history.push(clamp01(value));
  if (history.length > limit) {
    history.splice(0, history.length - limit);
  }
}

/** The loudest value of the visible history, for the readout */
export function signalPeak(history: number[]): number {
  let peak = 0;
  for (const value of history) {
    if (value > peak) {
      peak = value;
    }
  }
  return peak;
}

/**
 * Whether one cell of a column is lit. The cells below the top of the column are solid, and
 * the top edge dissolves through the Bayer matrix, so the column's crest dithers instead of
 * ending in a straight line. `phase` steps the matrix over time, as the kit's spectrogram does
 */
export function cellLit(
  row: number,
  rows: number,
  value: number,
  column: number,
  phase: number,
): boolean {
  const level = clamp01(value);
  const lit = litRows(level, rows);
  if (row < 0 || row >= lit) {
    return false;
  }
  const density = level - row / rows;
  return density * 1.4 > BAYER[row & 7][(column + phase) & 7];
}

/** The tone of a lit cell: the crest is the live tone, the body the accent tone */
export function cellTone(row: number, rows: number, value: number): "peak" | "lit" {
  const lit = litRows(clamp01(value), rows);
  return row >= lit - 2 ? "peak" : "lit";
}
