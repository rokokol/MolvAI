// SPDX-License-Identifier: MIT
// Phrase history: search, filters, copy, paste again and delete.
// The rows and filters live in HistoryProvider, so leaving the tab keeps them.

import { useEffect } from "react";

import { api, copyToClipboard } from "../api";
import { entryTag, formatLatency, formatTimestamp, formatWpm } from "../format";
import { useHistoryState } from "../history/HistoryProvider";
import { useI18n } from "../i18n";

export default function History() {
  const { t, lang } = useI18n();
  const {
    entries,
    apps,
    query,
    setQuery,
    app,
    setApp,
    since,
    setSince,
    until,
    setUntil,
    loading,
    filtered,
    dataDir,
    copiedId,
    setCopiedId,
    reload,
    reloadIfStale,
    remove,
    clearAll,
  } = useHistoryState();

  // A phrase recorded while another tab was open is worth fetching on return
  useEffect(() => {
    reloadIfStale();
  }, [reloadIfStale]);

  const clearAllConfirmed = async () => {
    if (!window.confirm(t("history.confirmClear"))) {
      return;
    }
    await clearAll();
  };

  return (
    <>
      <h1>{t("history.title")}</h1>

      <section className="card">
        <div className="grid2">
          <div className="field">
            <label htmlFor="history-search">{t("history.search")}</label>
            <input
              id="history-search"
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="history-app">{t("history.app")}</label>
            <select
              id="history-app"
              value={app}
              onChange={(event) => setApp(event.target.value)}
            >
              <option value="">{t("history.allApps")}</option>
              {apps.map((name) => (
                <option key={name} value={name}>
                  {name}
                </option>
              ))}
            </select>
          </div>
          <div className="field">
            <label htmlFor="history-since">{t("history.since")}</label>
            <input
              id="history-since"
              type="date"
              value={since}
              onChange={(event) => setSince(event.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="history-until">{t("history.until")}</label>
            <input
              id="history-until"
              type="date"
              value={until}
              onChange={(event) => setUntil(event.target.value)}
            />
          </div>
        </div>
        <div className="row spread">
          <span className="small muted">{t("history.count", { count: entries.length })}</span>
          <div className="row">
            <button type="button" onClick={reload}>
              {t("common.refresh")}
            </button>
            {dataDir && (
              <button type="button" onClick={() => void api.openPath(dataDir)}>
                {t("common.openFolder")}
              </button>
            )}
            <button
              type="button"
              className="danger"
              onClick={() => void clearAllConfirmed()}
              disabled={entries.length === 0}
            >
              {t("common.clear")}
            </button>
          </div>
        </div>
      </section>

      {loading && entries.length === 0 ? (
        <p className="muted">{t("common.loading")}</p>
      ) : entries.length === 0 ? (
        <p className="empty">{filtered ? t("history.emptyFiltered") : t("history.empty")}</p>
      ) : (
        <ul className="entry-list">
          {entries.map((entry) => {
            const text = entry.text_final ?? entry.text_raw ?? null;
            const source = entryTag(entry);
            return (
              <li key={entry.id} className="entry">
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
                    onClick={async () => {
                      if (text && (await copyToClipboard(text))) {
                        setCopiedId(entry.id);
                      }
                    }}
                  >
                    {copiedId === entry.id ? t("common.copied") : t("common.copy")}
                  </button>
                  <button
                    type="button"
                    className="chip danger"
                    onClick={() => void remove(entry.id)}
                  >
                    {t("common.delete")}
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
