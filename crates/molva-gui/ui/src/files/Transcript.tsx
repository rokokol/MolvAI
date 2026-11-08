// SPDX-License-Identifier: MIT
// One transcript as a list of segments with their timecodes.
//
// A row is a button: it moves the player to the start of the segment. The search inside the
// transcript filters the rows, counts the matches and marks them, and the copy actions keep the
// whole text reachable with or without timecodes.

import { useId, useState } from "react";

import { copyToClipboard } from "../api";
import { formatElapsed, formatTimecode, renderFileResult, renderSegment } from "../format";
import { useI18n } from "../i18n";
import type { FileResult, Segment } from "../types";
import {
  activeSegmentIndex,
  highlightParts,
  matchingSegments,
  type MatchingSegment,
} from "./jobs";

export interface TranscriptProps {
  result: FileResult;
  /** Whether copied text carries the timecodes; the list follows the same choice */
  timecodes: boolean;
  /** The playback position, for the active row; absent while preview is disabled */
  currentMs?: number | null;
  /** Move the player to a segment; absent while preview is disabled */
  onSeek?: (ms: number) => void;
  /** Toggle the timecodes in the review; absent when the caller owns no such control */
  onTimecodesChange?: (value: boolean) => void;
  onOpen: (path: string) => void;
}

export default function Transcript({
  result,
  timecodes,
  currentMs,
  onSeek,
  onTimecodesChange,
  onOpen,
}: TranscriptProps) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [copied, setCopied] = useState<string | null>(null);
  const searchId = useId();
  const segments = result.segments ?? [];
  const { segments: shown, matches } = matchingSegments(segments, query);
  const active = activeSegmentIndex(segments, currentMs ?? null);

  const copyAll = async () => {
    if (await copyToClipboard(renderFileResult(result, timecodes))) {
      setCopied("all");
    }
  };

  const copyOne = async (segment: Segment, key: string) => {
    if (await copyToClipboard(renderSegment(segment, timecodes))) {
      setCopied(key);
    }
  };

  return (
    <div className="transcript">
      <div className="transcript-head">
        <span className="mono">{result.file}</span>
        <span className="small muted">
          {formatElapsed(result.audio_secs * 1000)} ·{" "}
          {t("files.segmentCount", { count: segments.length })}
        </span>
        {result.language && <span className="small muted">{result.language}</span>}
      </div>

      <div className="transcript-toolbar">
        <label className="visually-hidden" htmlFor={searchId}>
          {t("files.searchInTranscript")}
        </label>
        <input
          id={searchId}
          type="search"
          value={query}
          placeholder={t("files.searchInTranscript")}
          onChange={(event) => setQuery(event.target.value)}
        />
        <span className="small muted" role="status">
          {query.trim() !== ""
            ? t("files.matches", { count: matches })
            : t("files.segmentCount", { count: segments.length })}
        </span>
        <div className="row">
          <button type="button" className="chip" onClick={() => void copyAll()}>
            {copied === "all" ? t("common.copied") : t("files.copyAll")}
          </button>
          {onTimecodesChange && (
            <button
              type="button"
              className="chip"
              aria-pressed={timecodes}
              onClick={() => onTimecodesChange(!timecodes)}
            >
              {timecodes ? t("files.timecodesOn") : t("files.timecodesOff")}
            </button>
          )}
          <button type="button" className="chip" onClick={() => onOpen(result.file)}>
            {t("files.open")}
          </button>
        </div>
      </div>

      {segments.length === 0 ? (
        <div className="transcript-plain">
          <p className="small muted">{t("files.noSegments")}</p>
          <p className="text">{result.text}</p>
        </div>
      ) : shown.length === 0 ? (
        <p className="small muted">{t("files.noMatches")}</p>
      ) : (
        <ol className="segments">
          {shown.map((item) => (
            <SegmentRow
              key={item.index}
              item={item}
              query={query}
              active={item.index === active}
              showTimecodes={timecodes}
              onSeek={onSeek ? () => onSeek(item.segment.start_ms) : undefined}
              onCopy={() => void copyOne(item.segment, String(item.index))}
              copied={copied === String(item.index)}
            />
          ))}
        </ol>
      )}
    </div>
  );
}

function SegmentRow({
  item,
  query,
  active,
  showTimecodes,
  onSeek,
  onCopy,
  copied,
}: {
  item: MatchingSegment;
  query: string;
  active: boolean;
  showTimecodes: boolean;
  onSeek?: () => void;
  onCopy: () => void;
  copied: boolean;
}) {
  const { t } = useI18n();
  const { segment } = item;
  const parts = highlightParts(segment.text, query);
  const content = (
    <>
      {showTimecodes && (
        <span className="segment-times mono">
          <span>{formatTimecode(segment.start_ms)}</span>
          <span aria-hidden="true">→</span>
          <span>{formatTimecode(segment.end_ms)}</span>
          <span className="small muted">
            {formatElapsed(segment.end_ms - segment.start_ms)}
          </span>
        </span>
      )}
      <span className="segment-text">
        {parts.map((part, index) =>
          part.match ? (
            <mark key={index}>{part.text}</mark>
          ) : (
            <span key={index}>{part.text}</span>
          ),
        )}
      </span>
    </>
  );
  return (
    <li className={active ? "segment active" : "segment"}>
      {onSeek ? (
        <button
          type="button"
          className="segment-main"
          aria-current={active ? "true" : undefined}
          aria-label={t("files.seek", { time: formatTimecode(segment.start_ms) })}
          onClick={onSeek}
        >
          {content}
        </button>
      ) : (
        <div className="segment-main" aria-current={active ? "true" : undefined}>
          {content}
        </div>
      )}
      <button
        type="button"
        className="chip segment-copy"
        aria-label={t("files.copySegment", { time: formatTimecode(segment.start_ms) })}
        onClick={onCopy}
      >
        {copied ? t("common.copied") : t("common.copy")}
      </button>
    </li>
  );
}
