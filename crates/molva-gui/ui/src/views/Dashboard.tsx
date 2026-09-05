// SPDX-License-Identifier: MIT
// Панель: состояние демона, живой уровень сигнала, кнопки записи и последняя реплика.

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import { formatLatency, formatMinutes, formatWpm } from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { Entry, StatsSummary, StyleOption } from "../types";
import StateBadge from "../components/StateBadge";

interface Props extends ViewProps {
  level: number;
  zeroLevel: boolean;
  lastEntry: Entry | null;
  hypothesis: string;
}

export default function Dashboard({
  config,
  status,
  refreshStatus,
  reloadConfig,
  onError,
  level,
  zeroLevel,
  lastEntry,
  hypothesis,
}: Props) {
  const { t, lang } = useI18n();
  const [styles, setStyles] = useState<StyleOption[]>([]);
  const [today, setToday] = useState<StatsSummary | null>(null);
  const [copied, setCopied] = useState(false);

  const running = status?.daemon_running ?? false;
  const recording = status?.state === "recording";

  const loadToday = useCallback(async () => {
    try {
      setToday(await api.statsSummary(7));
    } catch (err) {
      onError(asCommandError(err));
    }
  }, [onError]);

  useEffect(() => {
    void loadToday();
    api.availableStyles().then(setStyles).catch(() => setStyles([]));
  }, [loadToday]);

  // Новая реплика меняет счётчики «сегодня»: перечитываем их сразу после события.
  useEffect(() => {
    if (lastEntry) {
      void loadToday();
    }
  }, [lastEntry, loadToday]);

  const run = async (action: () => Promise<unknown>) => {
    try {
      await action();
      onError(null);
      await refreshStatus();
    } catch (err) {
      onError(asCommandError(err));
    }
  };

  const chooseStyle = async (style: string) => {
    await run(() => api.setStyle(style));
    await reloadConfig();
  };

  const currentStyle = status?.style ?? config?.style.default ?? "";
  // Уровень редко доходит до единицы: корень растягивает тихую часть шкалы.
  const levelPercent = Math.min(100, Math.round(Math.sqrt(Math.max(0, level)) * 140));

  return (
    <>
      <h1>{t("dashboard.title")}</h1>

      <section className="card">
        <div className="row spread">
          <StateBadge state={status?.state ?? null} running={running} />
          <div className="row">
            {running ? (
              <>
                <button
                  type="button"
                  className="primary"
                  onClick={() =>
                    run(recording ? () => api.recordStop() : () => api.recordStart())
                  }
                >
                  {recording ? t("dashboard.stop") : t("dashboard.record")}
                </button>
                <button
                  type="button"
                  onClick={() => run(() => api.recordCancel())}
                  disabled={!recording}
                >
                  {t("dashboard.cancelRecording")}
                </button>
              </>
            ) : (
              <button
                type="button"
                className="primary"
                onClick={() => run(() => api.startDaemon())}
              >
                {t("daemon.start")}
              </button>
            )}
          </div>
        </div>
        {!running && (
          <div className="notice warning" role="status">
            <strong>{t("daemon.stopped")}</strong>
            <p>{status?.hint ?? t("daemon.hint")}</p>
          </div>
        )}
      </section>

      <section className="card">
        <h2>{t("dashboard.level")}</h2>
        <div
          className="meter"
          role="meter"
          aria-label={t("dashboard.level")}
          aria-valuenow={levelPercent}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div className="fill" style={{ width: `${levelPercent}%` }} />
        </div>
        {zeroLevel && (
          <div className="notice warning" role="alert">
            <strong>{t("dashboard.levelZero")}</strong>
            <p>{t("dashboard.levelZeroHint")}</p>
          </div>
        )}
        {hypothesis && (
          <p className="small muted" style={{ marginBottom: 0 }}>
            {t("dashboard.hypothesis")}: {hypothesis}
          </p>
        )}
      </section>

      {styles.length > 0 && (
        <section className="card">
          <h2>{t("dashboard.style")}</h2>
          <div className="row">
            {styles.map((style) => (
              <button
                key={style.id}
                type="button"
                className="chip"
                aria-pressed={style.id === currentStyle}
                onClick={() => void chooseStyle(style.id)}
              >
                {style.name}
              </button>
            ))}
          </div>
        </section>
      )}

      <section className="card">
        <h2>{t("dashboard.last")}</h2>
        {lastEntry ? (
          <article className="entry">
            <div className="meta">
              <span>{new Date(lastEntry.ts).toLocaleTimeString(lang === "en" ? "en-GB" : "ru-RU")}</span>
              <span>
                {formatWpm(lastEntry.wpm)} {t("common.wpm")}
              </span>
              <span>
                {t("dashboard.latency")}: {formatLatency(lastEntry.latency_ms.total)}
              </span>
              <span>{lastEntry.style}</span>
              {lastEntry.app && <span>{lastEntry.app}</span>}
            </div>
            <p className="text">
              {lastEntry.text_final ?? lastEntry.text_raw ?? t("history.noText")}
            </p>
            <div className="row">
              <button
                type="button"
                className="chip"
                onClick={async () => {
                  const text = lastEntry.text_final ?? lastEntry.text_raw ?? "";
                  setCopied(await copyToClipboard(text));
                }}
              >
                {copied ? t("common.copied") : t("common.copy")}
              </button>
              <button
                type="button"
                className="chip"
                disabled={!running}
                onClick={() =>
                  run(() =>
                    api.injectText(lastEntry.text_final ?? lastEntry.text_raw ?? ""),
                  )
                }
              >
                {t("history.pasteAgain")}
              </button>
            </div>
          </article>
        ) : (
          <p className="muted">{t("dashboard.lastEmpty")}</p>
        )}
      </section>

      <section className="card">
        <h2>{t("dashboard.today")}</h2>
        <dl className="tiles">
          <div className="tile">
            <dt>{t("dashboard.todayWords")}</dt>
            <dd>{today ? today.words_today : "—"}</dd>
          </div>
          <div className="tile">
            <dt>{t("dashboard.todayWpm")}</dt>
            <dd>
              {formatWpm(today?.avg_wpm_today)}
              <span className="unit">{t("common.wpm")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("dashboard.todayStreak")}</dt>
            <dd>
              {today?.streak_days ?? 0}
              <span className="unit">{t("common.days")}</span>
            </dd>
          </div>
          <div className="tile">
            <dt>{t("dashboard.todaySaved")}</dt>
            <dd>
              {formatMinutes(today?.saved_minutes ?? 0)}
              <span className="unit">{t("common.minutes")}</span>
            </dd>
          </div>
        </dl>
      </section>
    </>
  );
}
