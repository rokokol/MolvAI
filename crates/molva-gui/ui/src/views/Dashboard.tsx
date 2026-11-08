// SPDX-License-Identifier: MIT
// Dashboard: daemon state, live signal level, recording buttons and the last phrase

import { useCallback, useEffect, useState } from "react";

import { api, asCommandError, copyToClipboard } from "../api";
import {
  downloadPercent,
  formatLatency,
  formatMegabytes,
  formatMinutes,
  formatWpm,
  sourceLabelKey,
} from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { Entry, StatsSummary, StyleOption } from "../types";
import LevelSignal from "../components/LevelSignal";
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
  const lastSource = lastEntry ? sourceLabelKey(lastEntry.source) : null;

  const running = status?.daemon_running ?? false;
  const recording = status?.state === "recording";
  // A file transcription holds the recognizer: the badge and a notice say so instead of "Ready"
  const fileJob = status?.job === "file";
  const download = running ? undefined : status?.download;

  const loadToday = useCallback(async () => {
    try {
      setToday(await api.statsSummary(7));
    } catch (err) {
      onError(asCommandError(err));
    }
  }, [onError]);

  useEffect(() => {
    void loadToday();
  }, [loadToday]);

  // Built-in style names come translated from the Rust side: ask again when the language changes
  useEffect(() => {
    api.availableStyles().then(setStyles).catch(() => setStyles([]));
  }, [lang]);

  // A new phrase changes the "today" counters: we read them again right after the event
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
  const llmEnabled = config?.llm.enabled ?? false;

  return (
    <>
      <h1>{t("dashboard.title")}</h1>

      <section className={`card record-panel${recording ? " recording" : ""}`}>
        <div className="row spread">
          <div className="row record-state">
            <span
              className={`record-led ${recording ? "live" : running ? "ready" : "idle"}`}
              aria-hidden="true"
            />
            <StateBadge
              state={status?.state ?? null}
              running={running}
              job={status?.job ?? null}
            />
          </div>
          <div className="row actions-row">
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
              // A recording needs the daemon: the button starts it and records at once, so a
              // cold window is not a reason to say "the daemon is stopped" and stop there.
              // During a download the socket does not exist yet, and the single-instance check
              // would let a second daemon write to the same .part file
              !download && (
                <>
                  <button
                    type="button"
                    className="primary"
                    onClick={() => run(() => api.recordStart())}
                  >
                    {t("dashboard.record")}
                  </button>
                  <button
                    type="button"
                    onClick={() => run(() => api.startDaemon())}
                  >
                    {t("daemon.start")}
                  </button>
                </>
              )
            )}
          </div>
        </div>
        {fileJob && (
          <div className="notice" role="status">
            <strong>{t("dashboard.fileJob")}</strong>
            <p>{t("dashboard.fileJobHint")}</p>
          </div>
        )}
        {download && (
          <div className="notice" role="status">
            <strong>{t("daemon.downloading", { model: download.model })}</strong>
            <div
              className="meter"
              role="progressbar"
              aria-valuenow={downloadPercent(download)}
              aria-valuemin={0}
              aria-valuemax={100}
            >
              <div className="fill" style={{ width: `${downloadPercent(download)}%` }} />
            </div>
            <p>
              {t("daemon.downloadProgress", {
                done: formatMegabytes(download.downloaded),
                total: formatMegabytes(download.total),
                percent: String(downloadPercent(download)),
              })}
            </p>
            <p>{t("daemon.downloadHint")}</p>
          </div>
        )}
        {!running && !download && status?.exit_reason && (
          <div className="notice error" role="alert">
            <strong>{t("daemon.crashed")}</strong>
            <p>{status.exit_reason}</p>
            <p>{t("daemon.crashedHint")}</p>
          </div>
        )}
        {!running && !download && !status?.exit_reason && (
          <div className="notice warning" role="status">
            <strong>{t("daemon.stopped")}</strong>
            <p>{status?.hint ?? t("daemon.hint")}</p>
          </div>
        )}
      </section>

      <section className="card">
        <h2>{t("dashboard.level")}</h2>
        <LevelSignal level={level} active={recording} />
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
                disabled={!llmEnabled && style.id !== "verbatim"}
                title={
                  !llmEnabled && style.id !== "verbatim"
                    ? t("dashboard.styleNeedsLlm")
                    : undefined
                }
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
              {lastSource ? (
                <span className="badge">{t(lastSource)}</span>
              ) : (
                lastEntry.llm_used && (
                  <span className="badge">
                    LLM · {lastEntry.llm_provider ?? "?"}
                    {lastEntry.llm_model ? ` / ${lastEntry.llm_model}` : ""}
                  </span>
                )
              )}
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
            </div>
          </article>
        ) : (
          <p className="empty">{t("dashboard.lastEmpty")}</p>
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
