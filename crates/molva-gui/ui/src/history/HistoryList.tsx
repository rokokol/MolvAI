// SPDX-License-Identifier: MIT
// The rows of the history: one phrase each, with its date, its numbers, its text and the two
// actions it offers.
//
// The journal of a person who dictates every day holds thousands of phrases, and the list
// renders again on every letter typed into the search box and on every copy. A row is therefore
// its own component behind `memo`, and the actions of the list keep their identity, so a change
// that touches one phrase redraws one row.
//
// The rows arrive in batches too: painting a whole journal at once blocks the window for the
// length of one commit, and the list is worthless until that commit ends

import { memo, useCallback, useEffect, useState } from "react";

import { copyToClipboard } from "../api";
import { entryTag, formatLatency, formatTimestamp, formatWpm } from "../format";
import { useI18n, type Lang } from "../i18n";
import type { Entry } from "../types";

/** Rows painted by one commit; the rest follows in the pauses between them */
const BATCH = 200;

export interface HistoryListProps {
  entries: Entry[];
  /** The phrase whose copy button says so */
  copiedId: string | null;
  onCopied: (id: string) => void;
  onDelete: (id: string) => void;
}

export default function HistoryList({
  entries,
  copiedId,
  onCopied,
  onDelete,
}: HistoryListProps) {
  const { lang } = useI18n();
  // A list that grows keeps what it has painted; a list from another query starts again
  const [painted, setPainted] = useState({ list: entries, count: BATCH });
  if (painted.list !== entries) {
    setPainted({ list: entries, count: BATCH });
  }
  useEffect(() => {
    if (painted.count >= entries.length) {
      return;
    }
    const timer = window.setTimeout(
      () => setPainted((current) => ({ ...current, count: current.count + BATCH })),
      0,
    );
    return () => window.clearTimeout(timer);
  }, [painted, entries.length]);
  const shown = entries.length <= painted.count ? entries : entries.slice(0, painted.count);
  return (
    <ul className="entry-list">
      {shown.map((entry) => (
        <HistoryRow
          key={entry.id}
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
  entry,
  lang,
  copied,
  onCopied,
  onDelete,
}: {
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
    <li className="entry">
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
          entry.llm_used && (
            <span className="badge">
              LLM · {entry.llm_provider ?? "?"}
              {entry.llm_model ? ` / ${entry.llm_model}` : ""}
            </span>
          )
        )}
      </div>
      <p className={text ? "text" : "text muted"}>{text ?? t("history.noText")}</p>
      <div className="row">
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
    </li>
  );
});
