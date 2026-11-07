// SPDX-License-Identifier: MIT
// Search across the transcriptions of audio and video files: a query, the file, the
// timecode of the segment that holds the match, and a copy button. The search itself runs
// in Rust over the journal, so the CLI and this view return the same hits.

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { formatTimestamp } from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { SearchHit } from "../types";

export default function Search({ onError }: ViewProps) {
  const { t, lang } = useI18n();
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

  const line = (hit: SearchHit) =>
    hit.timecode ? `[${hit.timecode}] ${hit.text}` : hit.text;

  return (
    <>
      <h1>{t("search.title")}</h1>

      <section className="card">
        <div className="row" style={{ alignItems: "flex-end" }}>
          <div className="field" style={{ marginBottom: 0, flex: "1 1 20rem" }}>
            <label htmlFor="search-query">{t("search.query")}</label>
            <input
              id="search-query"
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
            <label htmlFor="search-limit">{t("search.limit")}</label>
            <input
              id="search-limit"
              type="number"
              min={0}
              value={limit}
              onChange={(event) => setLimit(Math.max(0, Number(event.target.value) || 0))}
            />
          </div>
          <button type="button" className="primary" onClick={() => void load()}>
            {t("search.find")}
          </button>
        </div>
        <p className="small muted">{t("search.hint")}</p>
      </section>

      {loading ? (
        <p className="muted">{t("common.loading")}</p>
      ) : hits.length === 0 ? (
        <p className="muted">
          {query.trim() ? t("search.emptyFiltered") : t("search.empty")}
        </p>
      ) : (
        <ul className="entry-list">
          {hits.map((hit, index) => {
            const key = `${hit.entry_id}-${hit.start_ms ?? 0}-${index}`;
            return (
              <li key={key} className="entry">
                <div className="meta">
                  <span className="mono">{hit.file || t("search.unknownFile")}</span>
                  {hit.timecode && <span className="mono">{hit.timecode}</span>}
                  <span>{formatTimestamp(hit.recorded_at, lang)}</span>
                  {hit.language && <span>{hit.language}</span>}
                </div>
                <p className="text">{hit.text}</p>
                <div className="row">
                  <button
                    type="button"
                    className="chip"
                    onClick={async () => {
                      if (await copyToClipboard(line(hit))) {
                        setCopied(key);
                      }
                    }}
                  >
                    {copied === key ? t("common.copied") : t("common.copy")}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </>
  );
}
