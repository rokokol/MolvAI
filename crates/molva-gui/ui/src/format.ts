// SPDX-License-Identifier: MIT
// Чистые функции представления и геометрия графика.
// Всё, что можно посчитать в ядре, считается в Rust; здесь только форматирование.

import type { DayPoint } from "./types";

/** Темп с одним знаком после запятой; отсутствующее значение — прочерк. */
export function formatWpm(value: number | null | undefined, dash = "—"): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return dash;
  }
  return value.toFixed(1);
}

/**
 * Разделитель разрядов — узкий неразрывный пробел U+202F: число не рвётся переносом
 * строки. Вынесен в константу, потому что на глаз он неотличим от обычного пробела,
 * и сравнивать с ним нужно именно эту константу, а не пробел из клавиатуры.
 */
export const GROUP_SEPARATOR = " ";

/** Целое с разделителями разрядов: 12345 → «12 345». */
export function formatCount(value: number): string {
  return Math.round(value)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, GROUP_SEPARATOR);
}

/** Минуты: до часа — числом, дальше «2:05»; меньше минуты — «<1». */
export function formatMinutes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "0";
  }
  if (value < 1) {
    return "<1";
  }
  const total = Math.round(value);
  const hours = Math.floor(total / 60);
  const minutes = total % 60;
  if (hours === 0) {
    return String(minutes);
  }
  return `${hours}:${String(minutes).padStart(2, "0")}`;
}

/** Задержка: до секунды в миллисекундах, дальше в секундах. */
export function formatLatency(ms: number | null | undefined, dash = "—"): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) {
    return dash;
  }
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  return `${(ms / 1000).toFixed(1)} s`;
}

/** Дата и время реплики в локальном формате пользователя. */
export function formatTimestamp(iso: string, lang: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return iso;
  }
  return date.toLocaleString(lang === "en" ? "en-GB" : "ru-RU", {
    dateStyle: "short",
    timeStyle: "short",
  });
}

/** Короткая подпись дня для оси графика: «05.09» из «2026-09-05». */
export function formatDayTick(day: string): string {
  const parts = day.split("-");
  if (parts.length !== 3) {
    return day;
  }
  return `${parts[2]}.${parts[1]}`;
}

/** Точка графика в координатах SVG. */
export interface ChartPoint {
  day: string;
  words: number;
  wpm: number | null;
  /** Левый край столбика и его ширина. */
  x: number;
  width: number;
  /** Верх столбика и его высота: ось слов растёт вверх. */
  y: number;
  height: number;
  /** Координата точки линии темпа; null — в этот день темпа не было. */
  wpmY: number | null;
}

export interface ChartGeometry {
  points: ChartPoint[];
  maxWords: number;
  maxWpm: number;
  /** Ломаная линии темпа; дни без темпа разрывают её новой командой `M`. */
  wpmPath: string;
}

/**
 * Геометрия графика: столбики слов по дням и линия среднего темпа.
 *
 * Обе шкалы независимы и начинаются от нуля, иначе столбики и линия визуально
 * спорят друг с другом. Пустой ряд даёт пустую геометрию, а не деление на ноль.
 */
export function chartGeometry(
  series: DayPoint[],
  width: number,
  height: number,
): ChartGeometry {
  const maxWords = Math.max(1, ...series.map((point) => point.words));
  const wpmValues = series
    .map((point) => point.avg_wpm)
    .filter((value): value is number => typeof value === "number");
  const maxWpm = Math.max(1, ...wpmValues);
  const slot = series.length > 0 ? width / series.length : width;
  const barWidth = Math.max(1, slot * 0.62);

  const points: ChartPoint[] = series.map((point, index) => {
    const barHeight = (point.words / maxWords) * height;
    const wpm = typeof point.avg_wpm === "number" ? point.avg_wpm : null;
    return {
      day: point.day,
      words: point.words,
      wpm,
      x: index * slot + (slot - barWidth) / 2,
      width: barWidth,
      y: height - barHeight,
      height: barHeight,
      wpmY: wpm === null ? null : height - (wpm / maxWpm) * height,
    };
  });

  const segments: string[] = [];
  let started = false;
  for (const point of points) {
    if (point.wpmY === null) {
      // Разрыв в данных не соединяем прямой: следующая точка начнёт линию заново.
      started = false;
      continue;
    }
    const x = point.x + point.width / 2;
    segments.push(`${started ? "L" : "M"}${x.toFixed(2)} ${point.wpmY.toFixed(2)}`);
    started = true;
  }

  return { points, maxWords, maxWpm, wpmPath: segments.join(" ") };
}

/** Ровные подписи оси: ноль, середина и максимум. */
export function axisTicks(max: number): number[] {
  return [0, Math.round(max / 2), Math.round(max)];
}
