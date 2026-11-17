// SPDX-License-Identifier: MIT
// The list of phrases as a window over the journal
//
// A journal of thousands of phrases is not a list of thousands of DOM nodes: only the rows on
// the screen and a few around them exist, and the rest of the journal is padding of the height
// the measured rows say it takes. A row that scrolls out is measured on its way, so the padding
// is exact where the list has been seen and estimated where it has not

import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { copyToClipboard } from "../api";
import { entryTag, formatLatency, formatTimestamp, formatWpm } from "../format";
import { useI18n, type Lang } from "../i18n";
import type { Entry } from "../types";
import { DEFAULT_ESTIMATE, rowsWindow } from "./window";

/** The layout effect is the one that measures after every paint; the server has none */
const useMeasureEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

/** Rows painted beyond the edges, so a scroll meets content instead of blank space */
const OVERSCAN = 4;

/** What the list assumes before it has measured the window it sits in */
const DEFAULT_VIEWPORT = 760;

export interface HistoryListProps {
  entries: Entry[];
  /** The phrase whose copy button says so */
  copiedId: string | null;
  onCopied: (id: string) => void;
  onDelete: (id: string) => void;
}

interface Viewport {
  scrollTop: number;
  height: number;
  gap: number;
  /** Distance of the top of the list from the top of the scrolling content */
  offset: number;
  width: number;
}

/** The element that scrolls the page, or `null` when the list is not in a document */
function scrollParent(element: HTMLElement | null): HTMLElement | null {
  let node = element?.parentElement ?? null;
  while (node !== null) {
    const overflow = window.getComputedStyle(node).overflowY;
    if (overflow === "auto" || overflow === "scroll") {
      return node;
    }
    node = node.parentElement;
  }
  return document.scrollingElement instanceof HTMLElement
    ? document.scrollingElement
    : null;
}

export default function HistoryList({
  entries,
  copiedId,
  onCopied,
  onDelete,
}: HistoryListProps) {
  const { lang } = useI18n();
  const list = useRef<HTMLUListElement | null>(null);
  // Measured heights by index; `0` means "not measured yet". The array belongs to the list that
  // is painted now, so another query starts from the estimate again
  const measured = useRef<{ list: Entry[]; heights: number[] }>({
    list: entries,
    heights: new Array(entries.length).fill(0),
  });
  if (measured.current.list !== entries) {
    measured.current = { list: entries, heights: new Array(entries.length).fill(0) };
  }
  const [viewport, setViewport] = useState<Viewport>({
    scrollTop: 0,
    height: DEFAULT_VIEWPORT,
    gap: 0,
    offset: 0,
    width: 0,
  });
  const [painted, setPainted] = useState(0);

  const readViewport = useCallback(() => {
    const element = list.current;
    if (element === null) {
      return;
    }
    const parent = scrollParent(element);
    if (parent === null) {
      return;
    }
    const style = window.getComputedStyle(element);
    const gap = Number.parseFloat(style.rowGap === "normal" ? "0" : style.rowGap) || 0;
    const parentBox = parent.getBoundingClientRect();
    const listBox = element.getBoundingClientRect();
    setViewport((current) => {
      const next: Viewport = {
        scrollTop: parent.scrollTop,
        height: parent.clientHeight,
        gap,
        offset: listBox.top - parentBox.top + parent.scrollTop,
        width: parent.clientWidth,
      };
      const same =
        Math.abs(next.scrollTop - current.scrollTop) < 1 &&
        Math.abs(next.height - current.height) < 1 &&
        Math.abs(next.gap - current.gap) < 0.5 &&
        Math.abs(next.offset - current.offset) < 1 &&
        Math.abs(next.width - current.width) < 1;
      return same ? current : next;
    });
  }, []);

  // The scroll and the size of the window belong to the page, not to the list
  useMeasureEffect(() => {
    const element = list.current;
    if (element === null) {
      return;
    }
    readViewport();
    const parent = scrollParent(element);
    const onScroll = () => readViewport();
    parent?.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onScroll);
    const observer =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(onScroll);
    if (observer !== null && parent !== null) {
      observer.observe(parent);
    }
    return () => {
      parent?.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onScroll);
      observer?.disconnect();
    };
  }, [readViewport]);

  // A painted row is measured once, and a height that differs from its estimate redraws the
  // padding; the comparison keeps the loop from settling into a redraw of itself
  useMeasureEffect(() => {
    const element = list.current;
    if (element === null) {
      return;
    }
    let changed = false;
    for (const child of element.children) {
      const index = Number((child as HTMLElement).dataset.index ?? "-1");
      if (index < 0 || index >= measured.current.heights.length) {
        continue;
      }
      const height = (child as HTMLElement).getBoundingClientRect().height;
      if (height > 0 && Math.abs(height - measured.current.heights[index]) > 0.5) {
        measured.current.heights[index] = height;
        changed = true;
      }
    }
    if (changed) {
      setPainted((count) => count + 1);
    }
  });

  const window_ = rowsWindow(measured.current.heights, {
    estimate: DEFAULT_ESTIMATE,
    gap: viewport.gap,
    scrollTop: Math.max(0, viewport.scrollTop - viewport.offset),
    viewport: viewport.height,
    overscan: OVERSCAN,
  });
  const shown = entries.slice(window_.start, window_.end);
  return (
    <ul
      className="entry-list"
      ref={list}
      style={{ paddingTop: window_.top, paddingBottom: window_.bottom }}
      data-painted={shown.length}
      data-version={painted}
    >
      {shown.map((entry, position) => (
        <HistoryRow
          key={entry.id}
          index={window_.start + position}
          entry={entry}
          lang={lang}
          copied={copiedId === entry.id}
          onCopied={onCopied}
          onDelete={onDelete}
        />
      ))}
    </ul>
  );
}

/** A row of the history: exported because a test holds it to its memo boundary */
export const HistoryRow = memo(function HistoryRow({
  index,
  entry,
  lang,
  copied,
  onCopied,
  onDelete,
}: {
  index: number;
  entry: Entry;
  lang: Lang;
  copied: boolean;
  onCopied: (id: string) => void;
  onDelete: (id: string) => void;
}) {
  const { t } = useI18n();
  const text = entry.text_final ?? entry.text_raw ?? null;
  const source = entryTag(entry);
  // The copy action keeps its identity, so a copy redraws one row and not the list
  const copy = useCallback(async () => {
    if (text && (await copyToClipboard(text))) {
      onCopied(entry.id);
    }
  }, [text, entry.id, onCopied]);
  return (
    <li className="entry" data-index={index}>
      {/* The actions own the left corner: a phrase of ten lines keeps its Copy where the eye
          left it, and the text beside them never runs under a button */}
      <div className="row entry-actions">
        <button
          type="button"
          className="chip"
          disabled={!text}
          onClick={() => void copy()}
        >
          {copied ? t("common.copied") : t("common.copy")}
        </button>
        <button type="button" className="chip danger" onClick={() => onDelete(entry.id)}>
          {t("common.delete")}
        </button>
      </div>
      <div className="entry-body">
      <div className="meta">
        <span>{formatTimestamp(entry.ts, lang)}</span>
        <span>
          {formatWpm(entry.wpm)} {t("common.wpm")}
        </span>
        <span>{formatLatency(entry.latency_ms.total)}</span>
        <span>{entry.style}</span>
        {entry.app && <span>{entry.app}</span>}
        <span>
          {entry.words} {t("common.words")}
        </span>
        {source ? (
          <span className="badge">{t(source)}</span>
        ) : (
          <span className="badge">
            STT · {entry.stt_model || entry.stt_engine}
          </span>
        )}
        {entry.llm_used && (
          <span className="badge">
            LLM · {entry.llm_provider ?? "?"}
            {entry.llm_model ? ` / ${entry.llm_model}` : ""}
          </span>
        )}
      </div>
      <p className={text ? "text" : "text muted"}>{text ?? t("history.noText")}</p>
      </div>
    </li>
  );
});
