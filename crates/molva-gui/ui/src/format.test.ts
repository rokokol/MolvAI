// SPDX-License-Identifier: MIT
// Тесты чистых функций представления: форматирование и геометрия графика.

import { describe, expect, it } from "vitest";

import {
  axisTicks,
  chartGeometry,
  formatCount,
  formatDayTick,
  formatLatency,
  formatMinutes,
  formatWpm,
  GROUP_SEPARATOR,
} from "./format";
import type { DayPoint } from "./types";

function day(date: string, words: number, avg_wpm: number | null): DayPoint {
  return { day: date, entries: 1, words, audio_secs: 10, avg_wpm, avg_latency_ms: 500 };
}

describe("formatWpm", () => {
  it("оставляет один знак после запятой", () => {
    expect(formatWpm(123.456)).toBe("123.5");
  });

  it("показывает прочерк вместо отсутствующего значения", () => {
    expect(formatWpm(null)).toBe("—");
    expect(formatWpm(undefined)).toBe("—");
    expect(formatWpm(Number.NaN)).toBe("—");
  });

  it("ноль — это значение, а не отсутствие", () => {
    expect(formatWpm(0)).toBe("0.0");
  });
});

describe("formatCount", () => {
  it("разделяет разряды узким неразрывным пробелом", () => {
    expect(GROUP_SEPARATOR.codePointAt(0)).toBe(0x202f);
    expect(formatCount(1118)).toBe(`1${GROUP_SEPARATOR}118`);
    expect(formatCount(1234567)).toBe(
      `1${GROUP_SEPARATOR}234${GROUP_SEPARATOR}567`,
    );
  });

  it("короткие числа не трогает", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
  });
});

describe("formatMinutes", () => {
  it("минуты меньше часа выводит числом", () => {
    expect(formatMinutes(7.4)).toBe("7");
  });

  it("больше часа — как часы и минуты", () => {
    expect(formatMinutes(125)).toBe("2:05");
  });

  it("доли минуты не округляет до нуля молча", () => {
    expect(formatMinutes(0.4)).toBe("<1");
    expect(formatMinutes(0)).toBe("0");
  });
});

describe("formatLatency", () => {
  it("до секунды — миллисекунды, дальше — секунды", () => {
    expect(formatLatency(450)).toBe("450 ms");
    expect(formatLatency(1500)).toBe("1.5 s");
  });

  it("отсутствующая стадия не превращается в ноль", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(undefined)).toBe("—");
  });
});

describe("formatDayTick", () => {
  it("оставляет от даты день и месяц", () => {
    expect(formatDayTick("2026-09-05")).toBe("05.09");
  });

  it("непонятную строку возвращает как есть", () => {
    expect(formatDayTick("вчера")).toBe("вчера");
  });
});

describe("chartGeometry", () => {
  const series = [day("2026-09-01", 100, 120), day("2026-09-02", 50, 60)];

  it("самый высокий столбик занимает всю высоту, остальные — пропорционально", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.maxWords).toBe(100);
    expect(geometry.points[0].height).toBeCloseTo(100);
    expect(geometry.points[1].height).toBeCloseTo(50);
  });

  it("столбики не налезают друг на друга", () => {
    const geometry = chartGeometry(series, 200, 100);
    const [first, second] = geometry.points;
    expect(first.x + first.width).toBeLessThanOrEqual(second.x);
  });

  it("ось слов растёт вверх: верх столбика тем выше, чем больше слов", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.points[0].y).toBeLessThan(geometry.points[1].y);
  });

  it("линия темпа строится по всем дням, где темп есть", () => {
    const geometry = chartGeometry(series, 200, 100);
    expect(geometry.wpmPath.startsWith("M")).toBe(true);
    expect(geometry.wpmPath).toContain("L");
  });

  it("день без темпа рвёт линию, а не соединяется прямой", () => {
    const withGap = [
      day("2026-09-01", 100, 120),
      day("2026-09-02", 10, null),
      day("2026-09-03", 80, 90),
    ];
    const geometry = chartGeometry(withGap, 300, 100);
    // Обе точки по краям разрыва начинают свой отрезок: ни одного L между ними.
    expect(geometry.points[1].wpmY).toBeNull();
    expect(geometry.wpmPath.match(/M/g)).toHaveLength(2);
    expect(geometry.wpmPath).not.toContain("L");
  });

  it("пустой ряд даёт пустую геометрию без деления на ноль", () => {
    const geometry = chartGeometry([], 200, 100);
    expect(geometry.points).toEqual([]);
    expect(geometry.wpmPath).toBe("");
    expect(Number.isFinite(geometry.maxWords)).toBe(true);
  });

  it("ряд из одних нулей не ломает масштаб", () => {
    const geometry = chartGeometry([day("2026-09-01", 0, null)], 200, 100);
    expect(geometry.maxWords).toBe(1);
    expect(geometry.points[0].height).toBe(0);
  });
});

describe("axisTicks", () => {
  it("даёт ноль, середину и максимум", () => {
    expect(axisTicks(100)).toEqual([0, 50, 100]);
  });
});
