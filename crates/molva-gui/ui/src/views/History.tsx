// SPDX-License-Identifier: MIT
// История реплик: поиск, фильтры, копирование, повторная вставка и удаление.

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { formatLatency, formatTimestamp, formatWpm } from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { Entry } from "../types";

/** Полночь указанной даты в ISO: фильтр «с даты» включает весь день. */
function dayStart(value: string): string | null {
  if (!value) {
    return null;
  }
  const date = new Date(`${value}T00:00:00`);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

function dayEnd(value: string): string | null {
  if (!value) {
    return null;
  }
  const date = new Date(`${value}T23:59:59.999`);
  return Number.isNaN(date.getTime()) ? null : date.toISOString();
}

export default function History({ status, onError }: ViewProps) {
  const { t, lang } = useI18n();
  const [entries, setEntries] = useState<Entry[]>([]);
  const [apps, setApps] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [app, setApp] = useState("");
  const [since, setSince] = useState("");
  const [until, setUntil] = useState("");
  const [loading, setLoading] = useState(true);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [dataDir, setDataDir] = useState("");

  const filtered = query.trim() !== "" || app !== "" || since !== "" || until !== "";

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const list = await api.historyList({
        query,
        app,
        since: dayStart(since),
        until: dayEnd(until),
      });
      setEntries(list);
      onError(null);
    } catch (err) {
      onError(asCommandError(err));
    } finally {
      setLoading(false);
    }
  }, [query, app, since, until, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    api.historyApps().then(setApps).catch(() => setApps([]));
    api.dataDirPath().then(setDataDir).catch(() => setDataDir(""));
  }, []);

  const remove = async (id: string) => {
    try {
      await api.historyDelete(id);
      setEntries((prev) => prev.filter((entry) => entry.id !== id));
    } catch (err) {
      onError(asCommandError(err));
    }
  };

  const clearAll = async () => {
    if (!window.confirm(t("history.confirmClear"))) {
      return;
    }
    try {
      await api.historyClear();
      setEntries([]);
    } catch (err) {
      onError(asCommandError(err));
    }
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
            <button type="button" onClick={() => void load()}>
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
              onClick={() => void clearAll()}
              disabled={entries.length === 0}
            >
              {t("common.clear")}
            </button>
          </div>
        </div>
      </section>

      {loading ? (
        <p className="muted">{t("common.loading")}</p>
      ) : entries.length === 0 ? (
        <p className="muted">{filtered ? t("history.emptyFiltered") : t("history.empty")}</p>
      ) : (
        <ul className="entry-list">
          {entries.map((entry) => {
            const text = entry.text_final ?? entry.text_raw ?? null;
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
                    className="chip"
                    disabled={!text || !status?.daemon_running}
                    onClick={() => {
                      if (text) {
                        api.injectText(text).catch((err) => onError(asCommandError(err)));
                      }
                    }}
                  >
                    {t("history.pasteAgain")}
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
