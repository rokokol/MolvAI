// SPDX-License-Identifier: MIT
// Статистика: плитки, график слов и темпа, таблица по приложениям, экспорт CSV.

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError } from "../api";
import {
  formatCount,
  formatLatency,
  formatMinutes,
  formatTimestamp,
  formatWpm,
} from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { StatsSummary } from "../types";
import WordsChart from "../components/WordsChart";

const RANGES = [7, 30, 90];

export default function Stats({ onError }: ViewProps) {
  const { t, lang } = useI18n();
  const [range, setRange] = useState(7);
  const [summary, setSummary] = useState<StatsSummary | null>(null);
  const [exported, setExported] = useState("");

  const load = useCallback(async () => {
    try {
      setSummary(await api.statsSummary(range));
      onError(null);
    } catch (err) {
      onError(asCommandError(err));
    }
  }, [range, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const exportCsv = async () => {
    try {
      setExported(await api.statsExportCsv(range));
    } catch (err) {
      onError(asCommandError(err));
    }
  };

  const reset = async () => {
    if (!window.confirm(t("stats.confirmReset"))) {
      return;
    }
    try {
      await api.historyClear();
      await load();
    } catch (err) {
      onError(asCommandError(err));
    }
  };

  if (!summary) {
    return (
      <>
        <h1>{t("stats.title")}</h1>
        <p className="muted">{t("common.loading")}</p>
      </>
    );
  }

  return (
    <>
      <h1>{t("stats.title")}</h1>

      <section className="card">
        <dl className="tiles">
          <div className="tile">
            <dt>{t("stats.totalWords")}</dt>
            <dd>{formatCount(summary.total_words)}</dd>
          </div>
          <div className="tile">
            <dt>{t("stats.wordsToday")}</dt>
            <dd>{formatCount(summary.words_today)}</dd>
          </div>
          <div className="tile">
            <dt>{t("stats.avgToday")}</dt>
            <dd>
              {formatWpm(summary.avg_wpm_today)}
              <span className="unit">{t("common.wpm")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("stats.avg7d")}</dt>
            <dd>
              {formatWpm(summary.avg_wpm_7d)}
              <span className="unit">{t("common.wpm")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("stats.avgAll")}</dt>
            <dd>
              {formatWpm(summary.avg_wpm_all)}
              <span className="unit">{t("common.wpm")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("stats.record")}</dt>
            <dd>
              {formatWpm(summary.record_wpm)}
              <span className="unit">{t("common.wpm")}</span>
            </dd>
            {summary.record_at && (
              <p className="small muted" style={{ margin: "0.2rem 0 0" }}>
                {t("stats.recordAt")}: {formatTimestamp(summary.record_at, lang)}
              </p>
            )}
          </div>
          <div className="tile">
            <dt>{t("stats.streak")}</dt>
            <dd>
              {summary.streak_days}
              <span className="unit">{t("common.days")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("stats.minutes")}</dt>
            <dd>
              {formatMinutes(summary.minutes_recorded)}
              <span className="unit">{t("common.minutes")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("stats.saved")}</dt>
            <dd>
              {formatMinutes(summary.saved_minutes)}
              <span className="unit">{t("common.minutes")}</span>
            </dd>
          </div>
        </dl>
      </section>

      <section className="card">
        <div className="row spread">
          <h2>{t("stats.chart")}</h2>
          <div className="row" role="group" aria-label={t("stats.chart")}>
            {RANGES.map((days) => (
              <button
                key={days}
                type="button"
                className="chip"
                aria-pressed={range === days}
                onClick={() => setRange(days)}
              >
                {t(`stats.range${days}`)}
              </button>
            ))}
          </div>
        </div>
        <WordsChart series={summary.series} />
      </section>

      <section className="card">
        <h2>{t("stats.latency")}</h2>
        <dl className="tiles">
          <div className="tile">
            <dt>{t("stats.latencyStt")}</dt>
            <dd>{formatLatency(summary.latency_ms.stt)}</dd>
          </div>
          <div className="tile">
            <dt>{t("stats.latencyLlm")}</dt>
            <dd>{formatLatency(summary.latency_ms.llm)}</dd>
          </div>
          <div className="tile">
            <dt>{t("stats.latencyInject")}</dt>
            <dd>{formatLatency(summary.latency_ms.inject)}</dd>
          </div>
          <div className="tile">
            <dt>{t("stats.latencyTotal")}</dt>
            <dd>{formatLatency(summary.latency_ms.total)}</dd>
          </div>
          <div className="tile">
            <dt>
              {t("stats.tokens")}: {t("stats.tokensPrompt")}
            </dt>
            <dd>{formatCount(summary.tokens.prompt)}</dd>
          </div>
          <div className="tile">
            <dt>
              {t("stats.tokens")}: {t("stats.tokensCompletion")}
            </dt>
            <dd>{formatCount(summary.tokens.completion)}</dd>
          </div>
        </dl>
      </section>

      <section className="card">
        <h2>{t("stats.byApp")}</h2>
        {summary.by_app.length === 0 ? (
          <p className="muted">{t("history.empty")}</p>
        ) : (
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>{t("history.app")}</th>
                  <th className="num">{t("stats.entriesColumn")}</th>
                  <th className="num">{t("common.words")}</th>
                  <th className="num">{t("common.wpm")}</th>
                </tr>
              </thead>
              <tbody>
                {summary.by_app.map((row) => (
                  <tr key={row.app}>
                    <td>{row.app}</td>
                    <td className="num">{row.entries}</td>
                    <td className="num">{formatCount(row.words)}</td>
                    <td className="num">{formatWpm(row.avg_wpm)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>

      <section className="card">
        <div className="row">
          <button type="button" onClick={() => void exportCsv()}>
            {t("stats.exportCsv")}
          </button>
          <button type="button" className="danger" onClick={() => void reset()}>
            {t("stats.resetStats")}
          </button>
        </div>
        {exported && (
          <div className="notice ok" role="status">
            {t("stats.exported", { path: exported })}
            <div className="row" style={{ marginTop: "0.4rem" }}>
              <button
                type="button"
                className="chip"
                onClick={() => void api.openPath(exported)}
              >
                {t("common.openFolder")}
              </button>
            </div>
          </div>
        )}
      </section>
    </>
  );
}
