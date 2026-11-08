// SPDX-License-Identifier: MIT
// Tests of the ambient motion's arithmetic: the trail's lifetime and its lattice, and the discs
// of the strip. What the canvas does with the answers is not checked here, and cannot be: the
// part a test can hold is the pace and the faintness

import { describe, expect, it } from "vitest";

import {
  liveSamples,
  pushSample,
  snapToCell,
  STRIP_PEAK,
  stripDiscs,
  TRAIL_CELL,
  TRAIL_LIFE_MS,
  TRAIL_MAX,
  trailStrength,
  type Sample,
} from "./ambient";

describe("pushSample", () => {
  it("keeps the newest samples and lets the oldest ones go", () => {
    const samples: Sample[] = [];
    for (let index = 0; index < TRAIL_MAX + 12; index += 1) {
      pushSample(samples, index, index * 2, index);
    }
    expect(samples).toHaveLength(TRAIL_MAX);
    expect(samples[0].x).toBe(12);
    expect(samples[samples.length - 1].at).toBe(TRAIL_MAX + 11);
  });

  it("takes a smaller cap when the caller names one", () => {
    const samples: Sample[] = [];
    pushSample(samples, 1, 1, 0, 2);
    pushSample(samples, 2, 2, 10, 2);
    pushSample(samples, 3, 3, 20, 2);
    expect(samples.map((sample) => sample.x)).toEqual([2, 3]);
  });
});

describe("liveSamples", () => {
  it("drops the samples that outlived the trail and keeps the rest in order", () => {
    const samples: Sample[] = [];
    pushSample(samples, 1, 1, 0);
    pushSample(samples, 2, 2, 100);
    pushSample(samples, 3, 3, 400);
    expect(liveSamples(samples, 410).map((sample) => sample.x)).toEqual([1, 2, 3]);
    expect(liveSamples(samples, 520).map((sample) => sample.x)).toEqual([2, 3]);
    expect(liveSamples(samples, 900).map((sample) => sample.x)).toEqual([3]);
    expect(liveSamples(samples, 1000)).toHaveLength(0);
  });

  it("leaves a sample exactly at the end of its life out of the trail", () => {
    const samples: Sample[] = [{ x: 1, y: 1, at: 0 }];
    expect(liveSamples(samples, TRAIL_LIFE_MS - 1)).toHaveLength(1);
    expect(liveSamples([{ x: 1, y: 1, at: 0 }], TRAIL_LIFE_MS)).toHaveLength(0);
  });
});

describe("trailStrength", () => {
  it("starts at the full ink and ends at none", () => {
    expect(trailStrength(0)).toBe(1);
    expect(trailStrength(TRAIL_LIFE_MS)).toBe(0);
    expect(trailStrength(TRAIL_LIFE_MS * 4)).toBe(0);
    expect(trailStrength(-20)).toBe(1);
  });

  it("fades without ever rising", () => {
    const ages = [0, 30, 90, 180, 300, 450, 519];
    const strengths = ages.map((age) => trailStrength(age));
    for (let index = 1; index < strengths.length; index += 1) {
      expect(strengths[index]).toBeLessThan(strengths[index - 1]);
    }
  });
});

describe("snapToCell", () => {
  it("lands a coordinate on the lattice", () => {
    expect(snapToCell(0)).toBe(0);
    expect(snapToCell(2)).toBe(0);
    expect(snapToCell(3)).toBe(TRAIL_CELL);
    expect(snapToCell(101)).toBe(100);
  });

  it("rounds to whole pixels when the caller asks for no lattice", () => {
    expect(snapToCell(7.4, 0)).toBe(7);
  });
});

describe("stripDiscs", () => {
  const width = 1660;
  const height = 112;

  it("keeps every disc in the band and under the faintness ceiling", () => {
    for (const at of [0, 7_000, 31_000, 96_000]) {
      const discs = stripDiscs(width, height, at);
      expect(discs).toHaveLength(3);
      for (const disc of discs) {
        expect(disc.cx).toBeGreaterThan(0);
        expect(disc.cx).toBeLessThan(width);
        expect(disc.r).toBeGreaterThan(0);
        expect(disc.peak).toBeGreaterThan(0);
        expect(disc.peak).toBeLessThanOrEqual(STRIP_PEAK);
        expect(disc.phase % 8).toBe(disc.phase);
      }
      expect(new Set(discs.map((disc) => disc.tone)).size).toBe(3);
      expect(new Set(discs.map((disc) => disc.phase)).size).toBe(3);
    }
  });

  it("drifts, and drifts slowly: a tenth of a second barely moves it", () => {
    const now = stripDiscs(width, height, 20_000);
    const later = stripDiscs(width, height, 20_100);
    const far = stripDiscs(width, height, 40_000);
    for (let index = 0; index < now.length; index += 1) {
      expect(Math.abs(later[index].cx - now[index].cx)).toBeLessThan(2);
      expect(Math.abs(far[index].cx - now[index].cx)).toBeGreaterThan(0);
    }
  });

  it("scales with the box the strip is drawn in", () => {
    const wide = stripDiscs(900, 96, 5_000);
    const narrow = stripDiscs(480, 96, 5_000);
    expect(wide[1].cx).toBeGreaterThan(narrow[1].cx);
    // The radius follows the band's own height, not the window's width
    expect(wide[1].r).toBeCloseTo(narrow[1].r, 5);
  });
});
