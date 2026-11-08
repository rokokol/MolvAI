// SPDX-License-Identifier: MIT
// The queue of file transcriptions: the state model and the pure arithmetic around it.
// The Rust side hands each file to the daemon and says which stage it reached; everything
// countable about the queue is computed here, so the window never shows a share it cannot back
// with a count.

import type { FileResult, Segment } from "../types";

export type JobStatus =
  | "queued"
  | "running"
  | "done"
  | "failed"
  | "cancelled"
  | "interrupted";

/** The phases of one transcription, as `molva://transcribe-progress` reports them */
export type Phase = "starting" | "engine" | "finalizing";

export interface JobProgress {
  phase: Phase;
  /** `null` while the daemon reports no measurable share of the work */
  percent: number | null;
  /** What the last event said; the live value counts from `started_at` */
  elapsed_ms: number;
}

export interface Job {
  id: string;
  path: string;
  /** The last path component: the list shows the name, the title shows the whole path */
  name: string;
  status: JobStatus;
  /** Epoch milliseconds; `null` before the job starts */
  started_at: number | null;
  finished_at: number | null;
  progress: JobProgress | null;
  /** The failure text or the reason a job was interrupted */
  detail: string | null;
  results: FileResult[];
}

/** A request to move a player to a timecode; `seq` makes a repeated click a new request */
export interface SeekRequest {
  ms: number;
  seq: number;
}

/** The translation key of a status */
export function statusKey(status: JobStatus): string {
  return {
    queued: "files.statusQueued",
    running: "files.statusRunning",
    done: "files.statusDone",
    failed: "files.statusFailed",
    cancelled: "files.statusCancelled",
    interrupted: "files.statusInterrupted",
  }[status];
}

/** The translation key of a phase */
export function phaseKey(phase: Phase): string {
  return {
    starting: "files.phaseStarting",
    engine: "files.phaseEngine",
    finalizing: "files.phaseFinalizing",
  }[phase];
}

/** The badge tone of a status, so the same state looks the same everywhere */
export function statusTone(status: JobStatus): string {
  switch (status) {
    case "running":
      return "busy";
    case "done":
      return "ready";
    case "failed":
    case "interrupted":
      return "recording";
    default:
      return "idle";
  }
}

/** Finished states: a job in one of them no longer occupies the engine */
const TERMINAL: JobStatus[] = ["done", "failed", "cancelled", "interrupted"];

/** The last component of a path, on both separators */
export function baseName(path: string): string {
  const trimmed = path.trim().replace(/[\\/]+$/, "");
  const parts = trimmed.split(/[\\/]/);
  return parts[parts.length - 1] || trimmed;
}

export function isTerminal(status: JobStatus): boolean {
  return TERMINAL.includes(status);
}

/** A new job waits in the queue until the scheduler starts it */
export function newJob(id: string, path: string): Job {
  return {
    id,
    path,
    name: baseName(path),
    status: "queued",
    started_at: null,
    finished_at: null,
    progress: null,
    detail: null,
    results: [],
  };
}

export interface QueueSummary {
  total: number;
  queued: number;
  running: number;
  done: number;
  failed: number;
  cancelled: number;
  interrupted: number;
  /** Jobs in a terminal state */
  finished: number;
  /** Share of successfully finished jobs, a whole percent; 100 only when every job has a result */
  percent: number;
}

/** Counts of the queue and the share of jobs that reached a terminal state */
export function queueSummary(jobs: Job[]): QueueSummary {
  const summary: QueueSummary = {
    total: jobs.length,
    queued: 0,
    running: 0,
    done: 0,
    failed: 0,
    cancelled: 0,
    interrupted: 0,
    finished: 0,
    percent: 0,
  };
  for (const job of jobs) {
    if (job.status === "queued" || job.status === "running" || job.status === "done") {
      summary[job.status] += 1;
    } else if (job.status === "failed") {
      summary.failed += 1;
    } else if (job.status === "cancelled") {
      summary.cancelled += 1;
    } else {
      summary.interrupted += 1;
    }
    if (isTerminal(job.status)) {
      summary.finished += 1;
    }
  }
  if (summary.total > 0) {
    summary.percent = Math.round((summary.done / summary.total) * 100);
  }
  return summary;
}

/** How long a job has run: live for a running one, frozen after it ends */
export function elapsedMs(job: Job, now: number): number | null {
  if (job.status === "running" && job.started_at !== null) {
    return Math.max(0, now - job.started_at);
  }
  if (job.started_at !== null && job.finished_at !== null) {
    return Math.max(0, job.finished_at - job.started_at);
  }
  if (job.progress && job.progress.elapsed_ms > 0) {
    return job.progress.elapsed_ms;
  }
  return null;
}

/** The segment that holds the moment, or `null` in a pause between segments */
export function activeSegmentIndex(
  segments: Segment[] | undefined,
  ms: number | null,
): number | null {
  if (!segments || ms === null) {
    return null;
  }
  for (let index = 0; index < segments.length; index += 1) {
    const segment = segments[index];
    if (ms >= segment.start_ms && ms < segment.end_ms) {
      return index;
    }
  }
  return null;
}

/** How many times `query` occurs in `text`, ignoring case; an empty query counts nothing */
export function countMatches(text: string, query: string): number {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return 0;
  }
  const haystack = text.toLowerCase();
  let count = 0;
  let from = 0;
  for (;;) {
    const found = haystack.indexOf(needle, from);
    if (found < 0) {
      return count;
    }
    count += 1;
    from = found + needle.length;
  }
}

/** A run of text and whether it is part of a match */
export interface TextPart {
  text: string;
  match: boolean;
}

/** Split text into alternating plain and matching runs, for `<mark>` and the match count */
export function highlightParts(text: string, query: string): TextPart[] {
  const needle = query.trim();
  if (!needle) {
    return [{ text, match: false }];
  }
  const lower = text.toLowerCase();
  const lowerNeedle = needle.toLowerCase();
  const parts: TextPart[] = [];
  let from = 0;
  for (;;) {
    const found = lower.indexOf(lowerNeedle, from);
    if (found < 0) {
      break;
    }
    if (found > from) {
      parts.push({ text: text.slice(from, found), match: false });
    }
    parts.push({ text: text.slice(found, found + needle.length), match: true });
    from = found + needle.length;
  }
  if (from < text.length) {
    parts.push({ text: text.slice(from), match: false });
  }
  return parts;
}

/** The segments that contain the query, with their position in the result */
export interface MatchingSegment {
  index: number;
  segment: Segment;
}

/** Segments that match, and how many matches they hold in total */
export function matchingSegments(
  segments: Segment[],
  query: string,
): { segments: MatchingSegment[]; matches: number } {
  const needle = query.trim();
  if (!needle) {
    return {
      segments: segments.map((segment, index) => ({ index, segment })),
      matches: 0,
    };
  }
  const found: MatchingSegment[] = [];
  let matches = 0;
  segments.forEach((segment, index) => {
    const count = countMatches(segment.text, needle);
    if (count > 0) {
      found.push({ index, segment });
      matches += count;
    }
  });
  return { segments: found, matches };
}

// --- Persistence across a restart ---

/** The JSON payload kept in localStorage */
export const QUEUE_STORAGE_KEY = "molva.files.queue";

/** How many jobs a restart restores; older ones fall off the end */
export const MAX_STORED_JOBS = 25;

/** The stored payload of a queue larger than this shrinks by dropping the oldest result */
export const MAX_STORED_BYTES = 2_000_000;

/** The version of the stored shape; a different one is ignored instead of half-read */
const STORAGE_VERSION = 1;

interface StoredQueue {
  version: number;
  jobs: Job[];
}

/**
 * Serialize the queue for the next launch
 *
 * A running or queued job is stored as it is: on the next launch it becomes `interrupted`,
 * because the process that ran it died with the window. Results of finished jobs are kept
 * until the payload grows past `MAX_STORED_BYTES`, oldest first
 */
export function serializeQueue(jobs: Job[]): string {
  let kept = jobs.slice(0, MAX_STORED_JOBS);
  let payload: StoredQueue = { version: STORAGE_VERSION, jobs: kept };
  let text = JSON.stringify(payload);
  // Drop the oldest job that carries results; a job without results is worth keeping
  while (text.length > MAX_STORED_BYTES && kept.some((job) => job.results.length > 0)) {
    const lastWithResults = [...kept]
      .reverse()
      .find((job) => job.results.length > 0);
    if (!lastWithResults) {
      break;
    }
    kept = kept.map((job) =>
      job.id === lastWithResults.id ? { ...job, results: [] } : job,
    );
    payload = { version: STORAGE_VERSION, jobs: kept };
    text = JSON.stringify(payload);
  }
  return text;
}

function isJob(value: unknown): value is Job {
  if (!value || typeof value !== "object") {
    return false;
  }
  const job = value as Partial<Job>;
  return (
    typeof job.id === "string" &&
    typeof job.path === "string" &&
    typeof job.status === "string" &&
    Array.isArray(job.results)
  );
}

/**
 * Read the stored queue and mark every job that was still running as interrupted
 *
 * The window stopped listening when it closed, and the daemon kept no queue for it: the honest
 * status is "interrupted", never "done". A result that was never shown does not exist for the
 * queue, even if the daemon wrote it to the journal. The reason is not stored as text; the
 * interface supplies the localized sentence.
 */
export function hydrateQueue(raw: string | null): Job[] {
  if (!raw) {
    return [];
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!parsed || typeof parsed !== "object") {
    return [];
  }
  const stored = parsed as Partial<StoredQueue>;
  if (stored.version !== STORAGE_VERSION || !Array.isArray(stored.jobs)) {
    return [];
  }
  return stored.jobs.filter(isJob).slice(0, MAX_STORED_JOBS).map((job) => {
    // A running job lost its worker with the window. A queued job never started at all, so
    // calling it interrupted would invent a reason; it is simply cancelled.
    const lost = job.status === "running";
    const never_started = job.status === "queued";
    return {
      ...job,
      name: job.name || baseName(job.path),
      status: lost ? "interrupted" : never_started ? "cancelled" : job.status,
      progress: lost || never_started ? null : job.progress,
      finished_at: lost || never_started ? null : job.finished_at,
      detail: lost || never_started ? null : job.detail,
      results: lost || never_started ? [] : job.results,
    };
  });
}
