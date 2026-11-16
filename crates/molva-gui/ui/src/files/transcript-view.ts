// SPDX-License-Identifier: MIT
// The rows one transcript shows: the segments the query keeps, the number of matches, and the
// identity React needs to skip the rows that did not change.
//
// The transcript renders again on every step of the playback position, several times a second,
// while the answer to "which rows does this result show for this query" changes only when the
// result or the query does. The answer is kept per result, so one render of a long lecture costs
// one map lookup instead of a scan of every segment.

import { matchingSegments, type MatchingSegment } from "./jobs";
import type { Segment } from "../types";

export interface TranscriptView {
  /** The rows to show, in the order of the result */
  segments: MatchingSegment[];
  /** How many times the query occurs in the whole result */
  matches: number;
}

/** How many queries of one result are remembered: a person types a few, not thousands */
const QUERIES_PER_RESULT = 8;

/** The view of each result, keyed by the identity of its segments, so it dies with the result */
const views = new WeakMap<Segment[], Map<string, TranscriptView>>();

/** The rows of one result for one query, worked out once per pair */
export function transcriptView(segments: Segment[], query: string): TranscriptView {
  // The scan trims the query itself, so the trimmed text is the whole key
  const needle = query.trim();
  let byQuery = views.get(segments);
  if (!byQuery) {
    byQuery = new Map();
    views.set(segments, byQuery);
  }
  const known = byQuery.get(needle);
  if (known) {
    return known;
  }
  const scanned = matchingSegments(segments, needle);
  const view: TranscriptView = { segments: scanned.segments, matches: scanned.matches };
  if (byQuery.size >= QUERIES_PER_RESULT) {
    byQuery.clear();
  }
  byQuery.set(needle, view);
  return view;
}
