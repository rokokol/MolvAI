// SPDX-License-Identifier: MIT
// Pure presentation functions and chart geometry.
// The core in Rust computes everything it can compute. Here we only format

import type { DayPoint, DownloadProgress, FileResult, Segment } from "./types";

/** Pace with one decimal digit; a missing value becomes a dash */
export function formatWpm(value: number | null | undefined, dash = "—"): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return dash;
  }
  return value.toFixed(1);
}

/**
 * The digit separator is a narrow no-break space U+202F: a line break does not split
 * the number. It is a constant, because by eye it looks the same as a normal space.
 * Compare with this constant, not with a space from the keyboard
 */
export const GROUP_SEPARATOR = " ";

/** Integer with digit separators: 12345 becomes "12 345" */
export function formatCount(value: number): string {
  return Math.round(value)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, GROUP_SEPARATOR);
}

/** Share of the downloaded weights in percent; 100 only when the download is complete */
export function downloadPercent(download: DownloadProgress): number {
  if (download.total <= 0) {
    return 0;
  }
  return Math.min(100, Math.floor((download.downloaded / download.total) * 100));
}

/** Bytes in whole megabytes (MiB): 487 601 967 becomes "465" */
export function formatMegabytes(bytes: number): string {
  return formatCount(bytes / (1024 * 1024));
}

/** Minutes: below one hour as a number, above as "2:05"; below one minute as "<1" */
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

/** Latency: milliseconds below one second, seconds above */
export function formatLatency(ms: number | null | undefined, dash = "—"): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) {
    return dash;
  }
  if (ms < 1000) {
    return `${Math.round(ms)} ms`;
  }
  return `${(ms / 1000).toFixed(1)} s`;
}

/** Date and time of a phrase in the local format of the user */
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

/** Badge of a journal entry that did not come from the microphone, or null */
export function sourceLabelKey(source: string): string | null {
  if (source === "file") {
    return "history.files";
  }
  if (source === "mcp") {
    return "history.mcp";
  }
  return null;
}

/** Badge of a journal entry: a snippet wins over the source of the phrase */
export function entryTag(entry: { source: string; snippet?: boolean }): string | null {
  if (entry.snippet) {
    return "history.snippet";
  }
  return sourceLabelKey(entry.source);
}

/** Short label of a day for the chart axis: "05.09" from "2026-09-05" */
export function formatDayTick(day: string): string {
  const parts = day.split("-");
  if (parts.length !== 3) {
    return day;
  }
  return `${parts[2]}.${parts[1]}`;
}

/** Chart point in SVG coordinates */
export interface ChartPoint {
  day: string;
  words: number;
  wpm: number | null;
  /** Left edge of the bar and its width */
  x: number;
  width: number;
  /** Top of the bar and its height: the word axis grows upward */
  y: number;
  height: number;
  /** Coordinate of the pace line point; null means the day had no pace */
  wpmY: number | null;
}

export interface ChartGeometry {
  points: ChartPoint[];
  maxWords: number;
  maxWpm: number;
  /** Polyline of the pace line; days without a pace break it with a new `M` command */
  wpmPath: string;
}

/**
 * Chart geometry: bars of words per day and a line of the average pace.
 *
 * The two scales are independent and start at zero. Otherwise the bars and the line
 * visually disagree with each other. An empty series gives empty geometry, not a
 * division by zero
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
      // We do not join a gap in the data with a straight line: the next point starts the line again
      started = false;
      continue;
    }
    const x = point.x + point.width / 2;
    segments.push(`${started ? "L" : "M"}${x.toFixed(2)} ${point.wpmY.toFixed(2)}`);
    started = true;
  }

  return { points, maxWords, maxWpm, wpmPath: segments.join(" ") };
}

/** Round axis labels: zero, the middle and the maximum */
export function axisTicks(max: number): number[] {
  return [0, Math.round(max / 2), Math.round(max)];
}

/** A moment of the recording as `MM:SS.mmm`; minutes may pass 59 for a long file */
export function formatTimecode(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) {
    return "00:00.000";
  }
  const total = Math.floor(ms / 1000);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(Math.floor(ms) % 1000).padStart(3, "0")}`;
}

/** A span of time as minutes and seconds: 65 000 becomes "1:05" */
export function formatElapsed(ms: number | null | undefined): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms) || ms < 0) {
    return "—";
  }
  const total = Math.floor(ms / 1000);
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  const tail = `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  return hours > 0 ? `${hours}:${tail}` : tail;
}

/** One segment as copied text: with its timecodes when they are asked for */
export function renderSegment(segment: Segment, timecodes: boolean): string {
  const text = segment.text.trim();
  if (!timecodes) {
    return text;
  }
  return `[${formatTimecode(segment.start_ms)} → ${formatTimecode(segment.end_ms)}] ${text}`;
}

/** The text of a file transcription, with a timecode per segment when asked */
export function renderFileResult(result: FileResult, timecodes: boolean): string {
  if (!timecodes) {
    return result.text;
  }
  if (!result.segments || result.segments.length === 0) {
    return `[${formatTimecode(0)} → ${formatTimecode(result.audio_secs * 1000)}] ${result.text}`;
  }
  return result.segments.map((segment) => renderSegment(segment, timecodes)).join("\n");
}
