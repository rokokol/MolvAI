// SPDX-License-Identifier: MIT
// Tests of the pure presentation functions: formatting and chart geometry

import { describe, expect, it } from "vitest";

import {
  axisTicks,
  chartGeometry,
  downloadPercent,
  formatCount,
  formatDayTick,
  formatLatency,
  formatMegabytes,
  formatMinutes,
  formatWpm,
  GROUP_SEPARATOR,
} from "./format";
import type { DayPoint } from "./types";

function day(date: string, words: number, avg_wpm: number | null): DayPoint {
  return { day: date, entries: 1, words, audio_secs: 10, avg_wpm, avg_latency_ms: 500 };
}

describe("formatWpm", () => {
  it("keeps one decimal digit", () => {
    expect(formatWpm(123.456)).toBe("123.5");
  });

  it("shows a dash instead of a missing value", () => {
    expect(formatWpm(null)).toBe("—");
    expect(formatWpm(undefined)).toBe("—");
    expect(formatWpm(Number.NaN)).toBe("—");
  });

  it("treats zero as a value, not as a missing value", () => {
    expect(formatWpm(0)).toBe("0.0");
  });
});

describe("formatCount", () => {
  it("separates digit groups with a narrow no-break space", () => {
    expect(GROUP_SEPARATOR.codePointAt(0)).toBe(0x202f);
    expect(formatCount(1118)).toBe(`1${GROUP_SEPARATOR}118`);
    expect(formatCount(1234567)).toBe(
      `1${GROUP_SEPARATOR}234${GROUP_SEPARATOR}567`,
    );
  });

  it("leaves short numbers alone", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
  });
});

describe("formatMinutes", () => {
  it("shows minutes below one hour as a number", () => {
    expect(formatMinutes(7.4)).toBe("7");
  });

  it("shows more than one hour as hours and minutes", () => {
    expect(formatMinutes(125)).toBe("2:05");
  });

  it("does not silently round a fraction of a minute to zero", () => {
    expect(formatMinutes(0.4)).toBe("<1");
    expect(formatMinutes(0)).toBe("0");
  });
});

describe("formatLatency", () => {
  it("shows milliseconds below one second and seconds above", () => {
    expect(formatLatency(450)).toBe("450 ms");
    expect(formatLatency(1500)).toBe("1.5 s");
  });

  it("does not turn a missing stage into zero", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(undefined)).toBe("—");
  });
});

describe("formatDayTick", () => {
  it("keeps the day and the month of a date", () => {
    expect(formatDayTick("2026-09-05")).toBe("05.09");
  });

  it("returns an unparsable string as it is", () => {
    expect(formatDayTick("yesterday")).toBe("yesterday");
  });
});

describe("chartGeometry", () => {
  const series = [day("2026-09-01", 100, 120), day("2026-09-02", 50, 60)];

  it("gives the highest bar the full height and the others a proportional height", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.maxWords).toBe(100);
    expect(geometry.points[0].height).toBeCloseTo(100);
    expect(geometry.points[1].height).toBeCloseTo(50);
  });

  it("keeps the bars from overlapping", () => {
    const geometry = chartGeometry(series, 200, 100);
    const [first, second] = geometry.points;
    expect(first.x + first.width).toBeLessThanOrEqual(second.x);
  });

  it("grows the word axis upward: the more words, the higher the top of the bar", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.points[0].y).toBeLessThan(geometry.points[1].y);
  });

  it("builds the pace line over all days that have a pace", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.wpmPath.startsWith("M")).toBe(true);
    expect(geometry.wpmPath).toContain("L");
  });

  it("breaks the line at a day without a pace instead of joining with a straight line", () => {
    const withGap = [
      day("2026-09-01", 100, 120),
      day("2026-09-02", 10, null),
      day("2026-09-03", 80, 90),
    ];
    const geometry = chartGeometry(withGap, 300, 100);
    // Both points at the edges of the gap start their own segment: no L between them
    expect(geometry.points[1].wpmY).toBeNull();
    expect(geometry.wpmPath.match(/M/g)).toHaveLength(2);
    expect(geometry.wpmPath).not.toContain("L");
  });

  it("gives empty geometry for an empty series, without division by zero", () => {
    const geometry = chartGeometry([], 200, 100);
    expect(geometry.points).toEqual([]);
    expect(geometry.wpmPath).toBe("");
    expect(Number.isFinite(geometry.maxWords)).toBe(true);
  });

  it("keeps the scale intact for a series of zeros", () => {
    const geometry = chartGeometry([day("2026-09-01", 0, null)], 200, 100);
    expect(geometry.maxWords).toBe(1);
    expect(geometry.points[0].height).toBe(0);
  });
});

describe("downloadPercent", () => {
  it("computes the downloaded share as a whole percent", () => {
    expect(downloadPercent({ model: "small", downloaded: 120, total: 480 })).toBe(25);
  });

  it("keeps the percent within 0 to 100", () => {
    expect(downloadPercent({ model: "small", downloaded: 900, total: 480 })).toBe(100);
    expect(downloadPercent({ model: "small", downloaded: 10, total: 0 })).toBe(0);
  });
});

describe("formatMegabytes", () => {
  it("converts bytes to whole megabytes with a digit separator", () => {
    expect(formatMegabytes(487_601_967)).toBe("465");
    expect(formatMegabytes(3_095_033_483)).toBe(`2${GROUP_SEPARATOR}952`);
  });
});

describe("axisTicks", () => {
  it("gives zero, the middle and the maximum", () => {
    expect(axisTicks(100)).toEqual([0, 50, 100]);
  });
});
