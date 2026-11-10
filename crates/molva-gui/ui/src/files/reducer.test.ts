// SPDX-License-Identifier: MIT
// The races of the queue, settled by the reducer and checked without a window.
//
// A cancel is a decision of the window, and the daemon keeps working: the events it sends
// afterwards are late, and each one must leave the queue where the cancel put it. The last case
// is the double start a StrictMode render causes, which must be harmless.

import { describe, expect, it } from "vitest";

import { reducer, type Action } from "./FilesProvider";
import { newJob, type Job } from "./jobs";

const queued = newJob("job-1", "/media/talk.wav");
const running: Job = {
  ...queued,
  status: "running",
  started_at: 1_000,
  progress: { phase: "engine", percent: null, elapsed_ms: 0 },
};

function cancelled(job: Job): Job {
  return {
    ...job,
    status: "cancelled",
    finished_at: 5_000,
    detail: null,
  };
}

describe("the queue of file transcriptions", () => {
  it("does not revive a cancelled job when its progress arrives", () => {
    const action: Action = {
      type: "progress",
      id: "job-1",
      progress: { phase: "finalizing", percent: 100, elapsed_ms: 4_000 },
    };
    const next = reducer([cancelled(running)], action);
    expect(next[0].status).toBe("cancelled");
    expect(next[0].progress?.phase).toBe("engine");
  });

  it("keeps the answer that raced the cancel", () => {
    const action: Action = {
      type: "done",
      id: "job-1",
      at: 6_000,
      results: [
        {
          file: "/media/talk.wav",
          text: "привет",
          segments: [],
          audio_secs: 3,
          latency_ms: 10,
          language: "ru",
        },
      ],
    };
    const next = reducer([cancelled(running)], action);
    expect(next[0].status).toBe("done");
    expect(next[0].results).toHaveLength(1);
    expect(next[0].finished_at).toBe(6_000);
  });

  it("marks a queued job that failed as failed, never as done", () => {
    const action: Action = {
      type: "fail",
      id: "job-1",
      at: 7_000,
      status: "failed",
      detail: "the engine refused the file",
    };
    const next = reducer([queued], action);
    expect(next[0].status).toBe("failed");
    expect(next[0].detail).toBe("the engine refused the file");
    expect(next[0].results).toEqual([]);
  });

  it("leaves a finished job alone when a stale failure arrives", () => {
    const done: Job = { ...running, status: "done", finished_at: 3_000 };
    const next = reducer([done], {
      type: "fail",
      id: "job-1",
      at: 8_000,
      status: "failed",
      detail: "late",
    });
    expect(next[0]).toBe(done);
  });

  it("starts the same job twice without changing anything the second time", () => {
    const action: Action = { type: "start", id: "job-1", at: 1_000 };
    const once = reducer([queued], action);
    const twice = reducer(once, action);
    expect(twice).toEqual(once);
    expect(twice[0].status).toBe("running");
  });

  it("requeues a cancelled job with no trace of the previous attempt", () => {
    const next = reducer([cancelled(running)], { type: "requeue", id: "job-1" });
    expect(next[0].status).toBe("queued");
    expect(next[0].progress).toBeNull();
    expect(next[0].started_at).toBeNull();
    expect(next[0].results).toEqual([]);
  });
});
