// SPDX-License-Identifier: MIT
// Tests of the dashboard signal's arithmetic: the level-to-column mapping, the history the
// canvas keeps, and the dithering that decides which cells of a column are lit

import { describe, expect, it } from "vitest";

import {
  CELL,
  CELL_GAP,
  cellLit,
  cellTone,
  litRows,
  pushLevel,
  signalPeak,
  signalPercent,
  signalValue,
  SIGNAL_HISTORY,
} from "./level-signal";

describe("signalValue", () => {
  it("treats silence and a broken reading as zero", () => {
    expect(signalValue(0)).toBe(0);
    expect(signalValue(-0.3)).toBe(0);
    expect(signalValue(Number.NaN)).toBe(0);
  });

  it("lifts the quiet part of the range and stops at the top", () => {
    // RMS 0.25 is a normal phrase, not a quarter of a column
    expect(signalValue(0.25)).toBeCloseTo(0.7, 5);
    expect(signalValue(1)).toBe(1);
    expect(signalValue(4)).toBe(1);
  });

  it("grows with the level", () => {
    const levels = [0.004, 0.01, 0.04, 0.16, 0.36];
    const values = levels.map(signalValue);
    for (let index = 1; index < values.length; index += 1) {
      expect(values[index]).toBeGreaterThan(values[index - 1]);
    }
  });
});

describe("signalPercent", () => {
  it("is the readout of the same value", () => {
    expect(signalPercent(0)).toBe(0);
    expect(signalPercent(0.25)).toBe(70);
    expect(signalPercent(1)).toBe(100);
  });
});

describe("litRows", () => {
  it("leaves a silent column empty and fills a full one", () => {
    expect(litRows(0, 11)).toBe(0);
    expect(litRows(1, 11)).toBe(11);
    expect(litRows(2, 11)).toBe(11);
  });

  it("lights at least one cell for any audible level", () => {
    expect(litRows(0.001, 11)).toBe(1);
    expect(litRows(0.5, 10)).toBe(5);
  });

  it("never claims more cells than the lattice has", () => {
    for (const rows of [1, 7, 11, 0, -3]) {
      for (const value of [0, 0.01, 0.5, 0.99, 1]) {
        const lit = litRows(value, rows);
        expect(lit).toBeGreaterThanOrEqual(0);
        expect(lit).toBeLessThanOrEqual(Math.max(0, rows));
      }
    }
  });
});

describe("pushLevel", () => {
  it("keeps the events in the order they arrived", () => {
    const history: number[] = [];
    pushLevel(history, 0.2);
    pushLevel(history, 0.8);
    expect(history).toEqual([0.2, 0.8]);
  });

  it("drops the oldest column when the window is full", () => {
    const history: number[] = [];
    pushLevel(history, 0.2, 3);
    pushLevel(history, 0.4, 3);
    pushLevel(history, 0.6, 3);
    pushLevel(history, 0.8, 3);
    expect(history).toEqual([0.4, 0.6, 0.8]);
  });

  it("clamps a value that no level can produce", () => {
    const history: number[] = [];
    pushLevel(history, 3);
    pushLevel(history, -1);
    expect(history).toEqual([1, 0]);
  });

  it("remembers more columns than the widest window shows", () => {
    expect(SIGNAL_HISTORY).toBeGreaterThan(100);
  });
});

describe("signalPeak", () => {
  it("is zero for a signal that has not spoken yet", () => {
    expect(signalPeak([])).toBe(0);
  });

  it("is the loudest visible column", () => {
    expect(signalPeak([0.1, 0.7, 0.3])).toBe(0.7);
  });
});

describe("the lattice", () => {
  it("has a gap that keeps cells apart", () => {
    expect(CELL_GAP).toBeGreaterThan(0);
    expect(CELL_GAP).toBeLessThan(CELL);
  });

  it("never lights a cell above the column", () => {
    for (let row = 0; row < 12; row += 1) {
      for (const value of [0, 0.3, 0.8]) {
        const lit = litRows(value, 10);
        if (row >= lit || lit === 0) {
          expect(cellLit(row, 10, value, 0, 0)).toBe(false);
        }
      }
    }
  });

  it("dithers the crest more than the body", () => {
    const rows = 10;
    const value = 0.5;
    let bottom = 0;
    let crest = 0;
    for (let column = 0; column < 8; column += 1) {
      for (let phase = 0; phase < 8; phase += 1) {
        if (cellLit(0, rows, value, column, phase)) {
          bottom += 1;
        }
        if (cellLit(2, rows, value, column, phase)) {
          crest += 1;
        }
      }
    }
    expect(bottom).toBeGreaterThan(crest);
    expect(crest).toBeGreaterThan(0);
  });

  it("paints the top of a column in the live tone", () => {
    const rows = 10;
    const value = 0.5;
    const lit = litRows(value, rows);
    expect(cellTone(lit - 1, rows, value)).toBe("peak");
    expect(cellTone(lit - 2, rows, value)).toBe("peak");
    expect(cellTone(0, rows, value)).toBe("lit");
  });
});
