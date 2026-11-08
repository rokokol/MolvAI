// SPDX-License-Identifier: MIT
// Tests of the queue model: the counts the summary shows, the elapsed time, the segment that
// holds the playback position, the search inside a transcript and the storage of the queue

import { describe, expect, it } from "vitest";

import type { FileResult, Segment } from "../types";
import {
  activeSegmentIndex,
  baseName,
  countMatches,
  elapsedMs,
  highlightParts,
  hydrateQueue,
  MAX_STORED_BYTES,
  MAX_STORED_JOBS,
  matchingSegments,
  newJob,
  queueSummary,
  serializeQueue,
  type Job,
} from "./jobs";

function job(id: string, status: Job["status"], extra: Partial<Job> = {}): Job {
  return {
    ...newJob(id, `/media/${id}.wav`),
    status,
    ...extra,
  };
}

const result: FileResult = {
  file: "/media/a.wav",
  text: "привет мир",
  segments: [
    { start_ms: 0, end_ms: 1000, text: " привет" },
    { start_ms: 1200, end_ms: 2000, text: "мир и ещё мир" },
  ],
  audio_secs: 2,
  latency_ms: 10,
};

describe("baseName", () => {
  it("keeps the last component of a path", () => {
    expect(baseName("/home/u/media/talk.mp4")).toBe("talk.mp4");
    expect(baseName("C:\\media\\talk.mp4")).toBe("talk.mp4");
  });

  it("ignores a trailing separator and a bare name stays itself", () => {
    expect(baseName("/home/u/media/")).toBe("media");
    expect(baseName("talk.mp4")).toBe("talk.mp4");
  });
});

describe("queueSummary", () => {
  it("counts every state and computes the share of successful results", () => {
    const summary = queueSummary([
      job("a", "done"),
      job("b", "running"),
      job("c", "queued"),
    ]);
    expect(summary.total).toBe(3);
    expect(summary.done).toBe(1);
    expect(summary.running).toBe(1);
    expect(summary.queued).toBe(1);
    expect(summary.finished).toBe(1);
    expect(summary.percent).toBe(33);
  });

  it("never shows 100 percent while a job is still running", () => {
    expect(queueSummary([job("a", "done"), job("b", "running")]).percent).toBe(50);
  });

  it("does not call a failure a finished result", () => {
    expect(queueSummary([job("a", "done"), job("b", "failed")]).percent).toBe(50);
  });

  it("reaches 100 percent only when every job has a result", () => {
    expect(queueSummary([job("a", "done"), job("b", "done")]).percent).toBe(100);
  });

  it("does not count an interrupted or queued job as a result", () => {
    const summary = queueSummary([job("a", "interrupted"), job("b", "queued")]);
    expect(summary.interrupted).toBe(1);
    expect(summary.finished).toBe(1);
    expect(summary.percent).toBe(0);
  });

  it("gives zero of nothing rather than a division by zero", () => {
    const summary = queueSummary([]);
    expect(summary.total).toBe(0);
    expect(summary.percent).toBe(0);
  });
});

describe("elapsedMs", () => {
  it("counts from the start while the job runs", () => {
    const running = job("a", "running", { started_at: 1000 });
    expect(elapsedMs(running, 4000)).toBe(3000);
  });

  it("freezes at the end", () => {
    const done = job("a", "done", { started_at: 1000, finished_at: 2500 });
    expect(elapsedMs(done, 10_000)).toBe(1500);
  });

  it("has no value for a job that never started or was interrupted", () => {
    expect(elapsedMs(job("a", "queued"), 1000)).toBeNull();
    expect(elapsedMs(job("a", "interrupted", { started_at: 1000 }), 5000)).toBeNull();
  });
});

describe("activeSegmentIndex", () => {
  const segments: Segment[] = [
    { start_ms: 0, end_ms: 1000, text: "a" },
    { start_ms: 1200, end_ms: 2000, text: "b" },
  ];

  it("finds the segment that holds the moment", () => {
    expect(activeSegmentIndex(segments, 0)).toBe(0);
    expect(activeSegmentIndex(segments, 999)).toBe(0);
    expect(activeSegmentIndex(segments, 1200)).toBe(1);
  });

  it("returns nothing in a pause, at the end and without a position", () => {
    expect(activeSegmentIndex(segments, 1100)).toBeNull();
    expect(activeSegmentIndex(segments, 2000)).toBeNull();
    expect(activeSegmentIndex(segments, null)).toBeNull();
    expect(activeSegmentIndex(undefined, 100)).toBeNull();
  });
});

describe("countMatches", () => {
  it("counts occurrences of every case", () => {
    expect(countMatches("Мир и мир", "мир")).toBe(2);
    expect(countMatches("abc", "abc")).toBe(1);
  });

  it("does not count overlapping occurrences twice", () => {
    expect(countMatches("aaaa", "aa")).toBe(2);
  });

  it("counts nothing for an empty query or a missing one", () => {
    expect(countMatches("text", "  ")).toBe(0);
    expect(countMatches("text", "absent")).toBe(0);
  });
});

describe("highlightParts", () => {
  it("splits text into plain and matching runs", () => {
    expect(highlightParts("Мир и мир", "мир")).toEqual([
      { text: "Мир", match: true },
      { text: " и ", match: false },
      { text: "мир", match: true },
    ]);
  });

  it("keeps the text whole without a query", () => {
    expect(highlightParts("текст", "")).toEqual([{ text: "текст", match: false }]);
  });

  it("does not lose a trailing plain run", () => {
    expect(highlightParts("мир конец", "мир")).toEqual([
      { text: "мир", match: true },
      { text: " конец", match: false },
    ]);
  });
});

describe("matchingSegments", () => {
  const segments = result.segments ?? [];

  it("keeps the position of every matching segment and counts the matches", () => {
    const found = matchingSegments(segments, "мир");
    expect(found.segments.map((item) => item.index)).toEqual([1]);
    expect(found.matches).toBe(2);
  });

  it("returns every segment for an empty query and no match count", () => {
    const found = matchingSegments(segments, "");
    expect(found.segments).toHaveLength(2);
    expect(found.matches).toBe(0);
  });

  it("returns nothing when the transcript does not contain the query", () => {
    expect(matchingSegments(segments, "дождь").segments).toEqual([]);
  });
});

describe("the stored queue", () => {
  it("restores finished jobs with their results", () => {
    const jobs = [
      job("a", "done", { started_at: 1, finished_at: 2, results: [result] }),
    ];
    const restored = hydrateQueue(serializeQueue(jobs));
    expect(restored).toHaveLength(1);
    expect(restored[0].status).toBe("done");
    expect(restored[0].results).toHaveLength(1);
    expect(restored[0].results[0].segments).toHaveLength(2);
  });

  it("marks a job that was running when the window closed as interrupted", () => {
    const jobs = [job("a", "running", { started_at: 1, results: [result] })];
    const restored = hydrateQueue(serializeQueue(jobs));
    expect(restored[0].status).toBe("interrupted");
    expect(restored[0].results).toEqual([]);
    expect(restored[0].progress).toBeNull();
    expect(restored[0].detail).toBeNull();
  });

  it("marks a job that was queued but never started as cancelled", () => {
    const restored = hydrateQueue(serializeQueue([job("a", "queued")]));
    expect(restored[0].status).toBe("cancelled");
  });

  it("ignores a payload it does not understand instead of half-reading it", () => {
    expect(hydrateQueue(null)).toEqual([]);
    expect(hydrateQueue("not json")).toEqual([]);
    expect(hydrateQueue('{"version":99,"jobs":[]}')).toEqual([]);
    expect(hydrateQueue('{"version":1,"jobs":[{"id":1}]}')).toEqual([]);
  });

  it("keeps the newest jobs only", () => {
    const jobs = Array.from({ length: MAX_STORED_JOBS + 5 }, (_, index) =>
      job(`job-${index}`, "done"),
    );
    expect(hydrateQueue(serializeQueue(jobs))).toHaveLength(MAX_STORED_JOBS);
  });

  it("drops the oldest results when the payload would grow too large", () => {
    const huge: FileResult = { ...result, text: "x".repeat(1_200_000) };
    const jobs = [
      job("newest", "done", { results: [huge] }),
      job("oldest", "done", { results: [huge] }),
    ];
    const text = serializeQueue(jobs);
    expect(text.length).toBeLessThanOrEqual(MAX_STORED_BYTES);
    const restored = hydrateQueue(text);
    expect(restored).toHaveLength(2);
    expect(restored[0].results).toHaveLength(1);
    expect(restored[1].results).toEqual([]);
  });
});
