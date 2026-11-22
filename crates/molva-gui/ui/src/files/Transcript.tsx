// SPDX-License-Identifier: MIT
// One transcript as a list of segments with their timecodes.
//
// A row is a button: it moves the player to the start of the segment. The search inside the
// transcript filters the rows, counts the matches and marks them, and the copy actions keep the
// whole text reachable with or without timecodes.
//
// The position arrives several times a second while the file plays, and the whole transcript
// renders again on every step. A row is therefore its own component behind `memo`: only the two
// rows whose active flag flips are built again, and the list of rows for a query is worked out
// once per result instead of once per step.

import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";

import { copyToClipboard } from "../api";
import { formatElapsed, formatTimecode, renderFileResult, renderSegment, sttLabel } from "../format";
import { useI18n } from "../i18n";
import type { FileResult, Segment } from "../types";
import {
  activeSegmentIndex,
  highlightParts,
  type MatchingSegment,
} from "./jobs";
import { transcriptView } from "./transcript-view";

export interface TranscriptProps {
  result: FileResult;
  /** Whether copied text carries the timecodes; the list follows the same choice */
  timecodes: boolean;
  /** The playback position, for the active row; absent until the player reports one */
  currentMs?: number | null;
  /** Move the player to a segment; absent when the caller has no player */
  onSeek?: (ms: number) => void;
  /** Toggle the timecodes in the review; absent when the caller owns no such control */
  onTimecodesChange?: (value: boolean) => void;
  onOpen: (path: string) => void;
}

/** A result without segments keeps one shared empty list, so its view stays cached too */
const NO_SEGMENTS: Segment[] = [];

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
  const [copied, setCopied] = useState<number | null>(null);
  const searchId = useId();
  const segments = result.segments ?? NO_SEGMENTS;
  const view = transcriptView(segments, query);
  const active = activeSegmentIndex(segments, currentMs ?? null);
  // The row that plays is scrolled into view: a lecture of fifty segments is longer than the
  // window of the transcript, and following the sound by hand is not a job for the reader
  const activeRow = useRef<HTMLLIElement | null>(null);
  const lastActive = useRef<number | null>(null);
  useEffect(() => {
    if (active === null || active === lastActive.current) {
      return;
    }
    lastActive.current = active;
    activeRow.current?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const copyAll = async () => {
    if (await copyToClipboard(renderFileResult(result, timecodes))) {
      setCopied(-1);
    }
  };

  // One function for every row: a new closure per row would break the memo of the rows that
  // did not change
  const seekTo = useCallback(
    (ms: number) => {
      onSeek?.(ms);
    },
    [onSeek],
  );

  const copyOne = useCallback(
    async (index: number, segment: Segment) => {
      if (await copyToClipboard(renderSegment(segment, timecodes))) {
        setCopied(index);
      }
    },
    [timecodes],
  );

  // The rows are element objects, and React compares them by identity before it compares their
  // props: a step that stays inside one segment must not build ten thousand of them again. Only
  // the crossing into the next segment changes `active`, and only then is the list rebuilt
  const seekable = onSeek !== undefined;
  const rows = useMemo(
    () =>
      view.segments.map((item) => (
        <SegmentRow
          key={item.index}
          item={item}
          query={query}
          active={item.index === active}
          activeRef={item.index === active ? activeRow : undefined}
          showTimecodes={timecodes}
          seekable={seekable}
          onSeek={seekTo}
          onCopy={copyOne}
          copied={copied === item.index}
        />
      )),
    [view, query, active, timecodes, seekable, seekTo, copyOne, copied],
  );

  return (
    <div className="transcript">
      <div className="transcript-head">
        <span className="mono">{result.file}</span>
        <span className="small muted">
          {formatElapsed(result.audio_secs * 1000)} ·{" "}
          {t("files.segmentCount", { count: segments.length })}
        </span>
        {result.language && <span className="small muted">{result.language}</span>}
        {(result.stt_provider || result.stt_model) && (
          <span className="badge">STT · {sttLabel(result.stt_provider, result.stt_model)}</span>
        )}
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
            ? t("files.matches", { count: view.matches })
            : t("files.segmentCount", { count: segments.length })}
        </span>
        <div className="row">
          <button type="button" className="chip" onClick={() => void copyAll()}>
            {copied === -1 ? t("common.copied") : t("files.copyAll")}
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
      ) : view.segments.length === 0 ? (
        <p className="small muted">{t("files.noMatches")}</p>
      ) : (
        <ol className="segments">{rows}</ol>
      )}
    </div>
  );
}

/** A row of the transcript: exported because a test holds it to its memo boundary */
export const SegmentRow = memo(function SegmentRow({
  item,
  query,
  active,
  activeRef,
  showTimecodes,
  seekable,
  onSeek,
  onCopy,
  copied,
}: {
  item: MatchingSegment;
  query: string;
  active: boolean;
  /** Marks the active row, so the transcript can scroll it into view while the file plays */
  activeRef: React.Ref<HTMLLIElement> | undefined;
  showTimecodes: boolean;
  /** Whether the caller has a player to move: a row without one is plain text */
  seekable: boolean;
  onSeek: (ms: number) => void;
  onCopy: (index: number, segment: Segment) => void;
  copied: boolean;
}) {
  const { t } = useI18n();
  const { segment, index } = item;
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
        {parts.map((part, partIndex) =>
          part.match ? (
            <mark key={partIndex}>{part.text}</mark>
          ) : (
            <span key={partIndex}>{part.text}</span>
          ),
        )}
      </span>
    </>
  );
  // The row is a flex line, so the first child stands on the left: the copy of the segment
  // goes first, and a line however long cannot push it off the card
  const copy = (
    <button
      type="button"
      className="chip segment-copy"
      aria-label={t("files.copySegment", { time: formatTimecode(segment.start_ms) })}
      onClick={() => onCopy(index, segment)}
    >
      {copied ? t("common.copied") : t("common.copy")}
    </button>
  );

  // The words of a transcript exist to be selected, and a <button> refuses selection: the
  // WebKit window that draws this app paints a small frame instead of the highlight, and the
  // text runs out of it. The row that seeks is a div carrying the button's semantics itself,
  // and its click stands down when the click was the end of a selection drag
  const onSeekClick = (event: React.MouseEvent<HTMLDivElement>) => {
    const selection = window.getSelection();
    if (
      selection &&
      !selection.isCollapsed &&
      selection.containsNode(event.currentTarget, true)
    ) {
      return;
    }
    onSeek(segment.start_ms);
  };
  const onSeekKey = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onSeek(segment.start_ms);
    }
  };
  const seekProps = seekable
    ? {
        role: "button" as const,
        tabIndex: 0,
        "aria-label": t("files.seek", { time: formatTimecode(segment.start_ms) }),
        onClick: onSeekClick,
        onKeyDown: onSeekKey,
      }
    : {};
  const main = (
    <div
      className="segment-main"
      aria-current={active ? "true" : undefined}
      {...seekProps}
    >
      {content}
    </div>
  );
  return (
    <li className={active ? "segment active" : "segment"} ref={active ? activeRef : undefined}>
      {copy}
      {main}
    </li>
  );
});
