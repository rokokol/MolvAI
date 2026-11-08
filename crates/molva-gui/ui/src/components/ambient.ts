// SPDX-License-Identifier: MIT
// The arithmetic of the window's ambient motion, apart from the canvas: which pointer samples
// are still part of the trail, where a cell of the lattice falls, and where the discs of the
// strip drift. The canvas draws only what these functions return, so the calm pace and the
// faintness are values a test can hold

/** One pointer position, at the moment the browser reported it */
export interface Sample {
  x: number;
  y: number;
  at: number;
}

/** A disc of the strip: its centre, its radius, how dense it dithers and which role it takes */
export interface Disc {
  cx: number;
  cy: number;
  r: number;
  /** The share of the cells inside the disc that light at the centre */
  peak: number;
  /** Index of the role the disc takes from the strip's three */
  tone: 0 | 1 | 2;
  /** Steps the dithering matrix over the lattice, so two discs never light the same cells */
  phase: number;
}

/** How long a pointer sample stays in the trail */
export const TRAIL_LIFE_MS = 520;

/** How many samples the trail keeps: more than a pointer reports in one lifetime */
export const TRAIL_MAX = 36;

/** The side of one cell of the trail, in CSS pixels */
export const TRAIL_CELL = 5;

/** How much of the accent the trail puts on its newest cell */
export const TRAIL_INK = 0.62;

/** The side of one cell of the strip, in CSS pixels */
export const STRIP_CELL = 5;

/** How long one step of the strip's dithering matrix lasts */
export const STRIP_PHASE_MS = 220;

/**
 * The faintest peak a strip disc may use.
 *
 * Above this share the dithering stops reading as light in the background and starts covering
 * the page's own text
 */
export const STRIP_PEAK = 0.3;

/** Add a sample and drop the oldest beyond `max` */
export function pushSample(
  samples: Sample[],
  x: number,
  y: number,
  at: number,
  max: number = TRAIL_MAX,
): void {
  samples.push({ x, y, at });
  if (samples.length > max) {
    samples.splice(0, samples.length - max);
  }
}

/** The samples young enough to draw, oldest first; the expired ones leave the list */
export function liveSamples(
  samples: Sample[],
  now: number,
  life: number = TRAIL_LIFE_MS,
): Sample[] {
  let expired = 0;
  while (expired < samples.length && now - samples[expired].at >= life) {
    expired += 1;
  }
  if (expired > 0) {
    samples.splice(0, expired);
  }
  return samples;
}

/** How strongly a cell of the given age shows: the newest is full, the oldest is gone */
export function trailStrength(age: number, life: number = TRAIL_LIFE_MS): number {
  if (!Number.isFinite(age) || age <= 0) {
    return 1;
  }
  if (age >= life) {
    return 0;
  }
  const rest = 1 - age / life;
  return rest * rest;
}

/** The lattice cell that a coordinate belongs to, so a trail cell lands on a whole pixel */
export function snapToCell(value: number, cell: number = TRAIL_CELL): number {
  if (cell <= 0) {
    return Math.round(value);
  }
  return Math.round(value / cell) * cell;
}

/**
 * The three discs of the strip at `at` milliseconds.
 *
 * Each one drifts on a slow sine of its own, and the radii stay a little over the strip's own
 * height, so the light fills the band without ever becoming a shape that moves across it
 */
export function stripDiscs(width: number, height: number, at: number): Disc[] {
  const seconds = at / 1000;
  // The light sits above the band, so its density falls from the top edge downwards and no
  // part of the window gets a shape of its own
  const band = height;
  return [
    {
      cx: width * (0.18 + 0.05 * Math.sin(seconds * 0.11)),
      cy: -band * (0.35 + 0.08 * Math.cos(seconds * 0.09)),
      r: band * (1.5 + 0.12 * Math.sin(seconds * 0.13)),
      peak: STRIP_PEAK * 0.8,
      tone: 0,
      phase: 0,
    },
    {
      cx: width * (0.5 + 0.06 * Math.cos(seconds * 0.08)),
      cy: -band * (0.3 + 0.07 * Math.sin(seconds * 0.12)),
      r: band * (1.7 + 0.14 * Math.cos(seconds * 0.1)),
      peak: STRIP_PEAK * 0.65,
      tone: 1,
      phase: 3,
    },
    {
      cx: width * (0.82 + 0.05 * Math.sin(seconds * 0.14)),
      cy: -band * (0.4 + 0.06 * Math.sin(seconds * 0.07)),
      r: band * (1.35 + 0.1 * Math.sin(seconds * 0.09)),
      peak: STRIP_PEAK * 0.55,
      tone: 2,
      phase: 5,
    },
  ];
}
