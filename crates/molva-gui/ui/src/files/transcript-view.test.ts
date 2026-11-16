// SPDX-License-Identifier: MIT
// The rows of one transcript are worked out once per result and query.
//
// The playback position changes several times a second, and the transcript renders again on
// every change. A scan of the segments per render made a lecture of ten thousand segments
// unusable: the window redrew every row four times a second. These tests count the scans
// instead of trusting a stopwatch, so the day the count comes back the suite says so.

import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Segment } from "../types";

const counts = vi.hoisted(() => ({ scans: 0 }));

vi.mock("./jobs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./jobs")>();
  return {
    ...actual,
    matchingSegments: (segments: Segment[], query: string) => {
      counts.scans += 1;
      return actual.matchingSegments(segments, query);
    },
  };
});

import { matchingSegments } from "./jobs";
import { transcriptView } from "./transcript-view";

function segments(count: number, word = "слово"): Segment[] {
  return Array.from({ length: count }, (_, index) => ({
    start_ms: index * 1000,
    end_ms: index * 1000 + 900,
    text: ` сегмент ${index} ${word}`,
  }));
}

beforeEach(() => {
  counts.scans = 0;
});

describe("the view of one transcript", () => {
  it("scans the segments once for one query, however often the transcript renders", () => {
    const data = segments(10_000);

    for (let render = 0; render < 60; render += 1) {
      transcriptView(data, "слово");
    }

    expect(counts.scans).toBe(1);
  });

  it("hands back the same rows every time, so the rows below it can be skipped", () => {
    const data = segments(10);

    const first = transcriptView(data, "слово");

    expect(transcriptView(data, "слово")).toBe(first);
  });

  it("scans again for another query and for another result", () => {
    const data = segments(10);

    transcriptView(data, "слово");
    transcriptView(data, "сегмент");
    transcriptView(segments(10), "слово");

    expect(counts.scans).toBe(3);
  });

  it("keeps exactly what the scan below it returns", () => {
    const data = segments(50);

    for (const query of ["", "слово", "сегмент 7", "  слово  "]) {
      expect(transcriptView(data, query)).toEqual(matchingSegments(data, query));
    }
  });
});
