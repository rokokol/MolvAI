// SPDX-License-Identifier: MIT
// Search across every transcription, embedded in the Files tab.
//
// The search itself runs in Rust over the journal, so the CLI and the window return the same
// hits. A hit shows the file, the timecode of its segment and the text; when the file is still
// in the queue of this window, the hit opens that cell and moves its player to the timecode.

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { formatTimestamp } from "../format";
import { useI18n } from "../i18n";
import type { CommandError, SearchHit } from "../types";
import { useFilesQueue } from "./FilesProvider";
import { highlightParts } from "./jobs";

export interface TranscriptSearchProps {
  onError: (error: CommandError | null) => void;
}

export default function TranscriptSearch({ onError }: TranscriptSearchProps) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(20);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [loading, setLoading] = useState(true);
  const [copied, setCopied] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setHits(await api.searchFiles(query, limit));
      onError(null);
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setLoading(false);
    }
  }, [query, limit, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <section className="card" aria-labelledby="files-search-title">
      <h2 id="files-search-title">{t("search.title")}</h2>

      <div className="row" style={{ alignItems: "flex-end" }}>
        <div className="field" style={{ marginBottom: 0, flex: "1 1 20rem" }}>
          <label htmlFor="files-search-query">{t("search.query")}</label>
          <input
            id="files-search-query"
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                void load();
              }
            }}
          />
        </div>
        <div className="field" style={{ marginBottom: 0, width: "7rem" }}>
          <label htmlFor="files-search-limit">{t("search.limit")}</label>
          <input
            id="files-search-limit"
            type="number"
            min={0}
            value={limit}
            onChange={(event) =>
              setLimit(Math.max(0, Number(event.target.value) || 0))
            }
          />
        </div>
        <button type="button" className="primary" onClick={() => void load()}>
          {t("search.find")}
        </button>
      </div>
      <p className="small muted">{t("search.hint")}</p>

      {loading && hits.length === 0 ? (
        <p className="muted">{t("common.loading")}</p>
      ) : hits.length === 0 ? (
        <p className="empty">
          {query.trim() ? t("search.emptyFiltered") : t("search.empty")}
        </p>
      ) : (
        <ul className="entry-list">
          {hits.map((hit, index) => (
            <SearchHitCard
              key={`${hit.entry_id}-${hit.start_ms ?? 0}-${index}`}
              hit={hit}
              query={query}
              copied={copied === `${hit.entry_id}-${hit.start_ms ?? 0}-${index}`}
              onCopied={() => setCopied(`${hit.entry_id}-${hit.start_ms ?? 0}-${index}`)}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * One hit of the search: the file, the timecode, the fragment and the actions under it.
 *
 * Exported because a test holds this card to the plain stacked layout of `.entry`: the
 * two-column grid belongs to the history rows, and a hit that fell into it pushed its words
 * past the card, because the long path of the meta line sized the action column
 */
export function SearchHitCard({
  hit,
  query,
  copied,
  onCopied,
}: {
  hit: SearchHit;
  query: string;
  copied: boolean;
  onCopied: () => void;
}) {
  const { t, lang } = useI18n();
  const { holds, reveal } = useFilesQueue();
  const inQueue = holds(hit.file);
  const line = hit.timecode ? `[${hit.timecode}] ${hit.text}` : hit.text;
  return (
    <li className="entry">
      <div className="meta">
        <span className="mono">{hit.file || t("search.unknownFile")}</span>
        {hit.timecode && <span className="mono">{hit.timecode}</span>}
        <span>{formatTimestamp(hit.recorded_at, lang)}</span>
        {hit.language && <span>{hit.language}</span>}
      </div>
      <p className="text">
        {highlightParts(hit.text, query).map((part, partIndex) =>
          part.match ? (
            <mark key={partIndex}>{part.text}</mark>
          ) : (
            <span key={partIndex}>{part.text}</span>
          ),
        )}
      </p>
      <div className="row">
        <button
          type="button"
          className="chip"
          onClick={async () => {
            if (await copyToClipboard(line)) {
              onCopied();
            }
          }}
        >
          {copied ? t("common.copied") : t("common.copy")}
        </button>
        {inQueue && (
          <button
            type="button"
            className="chip"
            onClick={() => reveal(hit.file, hit.start_ms ?? null)}
          >
            {t("files.showInQueue")}
          </button>
        )}
      </div>
    </li>
  );
}
