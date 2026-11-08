// SPDX-License-Identifier: MIT
// Audio and video transcription: drag a file into the window or type a path, then review the
// result by timecode with its player or spectrogram.
//
// The queue itself lives in FilesProvider, above the tabs: this view renders it and keeps only
// what belongs to the input of the moment. Transcript search, global and inside one result, is
// here too, so the Files tab is the one place where transcriptions are reviewed.

import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { api, asCommandError } from "../api";
import { formatElapsed } from "../format";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import { useFilesQueue } from "../files/FilesProvider";
import JobCell from "../files/JobCell";
import { elapsedMs, queueSummary, statusKey, type JobStatus } from "../files/jobs";
import PathInput from "../files/PathInput";
import ProgressBar from "../files/ProgressBar";
import TranscriptSearch from "../files/TranscriptSearch";

/** The copy preference outlives the tab, because the review of a file does too */
const TIMECODES_KEY = "molva.files.timecodes";

function storedTimecodes(): boolean {
  try {
    const stored = localStorage.getItem(TIMECODES_KEY);
    return stored === null ? true : stored === "1";
  } catch {
    return true;
  }
}

export default function Files({ onError }: ViewProps) {
  const { t } = useI18n();
  const { jobs, dragging, start, clearFinished } = useFilesQueue();
  const [path, setPath] = useState("");
  const [timecodes, setTimecodes] = useState(storedTimecodes);

  // The elapsed time of a running job ticks here; the cells only render it
  const running = jobs.some((job) => job.status === "running");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) {
      setNow(Date.now());
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [running]);

  useEffect(() => {
    try {
      localStorage.setItem(TIMECODES_KEY, timecodes ? "1" : "0");
    } catch {
      // The choice does not persist, but the toggle works for this session
    }
  }, [timecodes]);

  const summary = queueSummary(jobs);
  const counts: { status: JobStatus; count: number }[] = [
    { status: "running", count: summary.running },
    { status: "queued", count: summary.queued },
    { status: "done", count: summary.done },
    { status: "failed", count: summary.failed },
    { status: "cancelled", count: summary.cancelled },
    { status: "interrupted", count: summary.interrupted },
  ];

  const openTarget = (target: string) =>
    api.openPath(target).catch((err) => onError(asCommandError(err)));

  const chooseFile = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: t("files.fileFilter"),
          extensions: [
            "mp3",
            "wav",
            "m4a",
            "m4b",
            "aac",
            "ogg",
            "oga",
            "opus",
            "flac",
            "mp4",
            "m4v",
            "mkv",
            "webm",
            "mov",
            "avi",
            "ts",
          ],
        },
      ],
    });
    if (typeof selected === "string" && selected.trim() !== "") {
      start(selected);
    }
  };

  return (
    <>
      <h1>{t("files.title")}</h1>

      <section className="card">
        <button
          type="button"
          className={dragging ? "dropzone over" : "dropzone"}
          onClick={() => void chooseFile()}
          title={t("files.chooseFile")}
        >
          {t("files.drop")}
        </button>
        <div className="row" style={{ marginTop: "0.75rem", alignItems: "flex-end" }}>
          <div className="field" style={{ marginBottom: 0, flex: "1 1 18rem" }}>
            <label htmlFor="files-path">{t("files.pathLabel")}</label>
            <PathInput
              id="files-path"
              value={path}
              onChange={setPath}
              onStart={() => {
                start(path);
                setPath("");
              }}
              suggest={(input) => api.pathSuggestions(input, 10)}
            />
          </div>
          <button
            type="button"
            className="primary"
            onClick={() => {
              start(path);
              setPath("");
            }}
            disabled={path.trim() === ""}
          >
            {t("files.add")}
          </button>
        </div>
        <div className="row spread">
          <label className="check" title={t("files.timecodesHint")}>
            <input
              type="checkbox"
              checked={timecodes}
              onChange={(event) => setTimecodes(event.target.checked)}
            />
            {t("files.timecodes")}
          </label>
          <span className="small muted">{t("files.hint")}</span>
        </div>
        <p className="small muted">{t("files.suggestionHint")}</p>
      </section>

      <section className="card">
        <div className="row spread">
          <h2>{t("files.queue")}</h2>
          <div className="row">
            <button
              type="button"
              className="chip"
              onClick={clearFinished}
              disabled={summary.finished === 0}
            >
              {t("files.clearFinished")}
            </button>
          </div>
        </div>

        {jobs.length === 0 ? (
          <p className="empty">{t("files.queueEmpty")}</p>
        ) : (
          <>
            <div className="queue-progress">
              <ProgressBar
                percent={summary.percent}
                tone={summary.done === summary.total ? "done" : "running"}
                label={t("files.queuePercent", {
                  percent: summary.percent,
                  done: summary.done,
                  total: summary.total,
                })}
              />
              <span className="small muted mono">
                {summary.done} / {summary.total} ·{" "}
                {formatElapsed(
                  jobs.reduce((total, job) => total + (elapsedMs(job, now) ?? 0), 0),
                )}
              </span>
            </div>
            <div className="row small muted queue-counts">
              {counts.map(
                (item) =>
                  item.count > 0 && (
                    <span key={item.status}>
                      {item.count} {t(statusKey(item.status))}
                    </span>
                  ),
              )}
            </div>

            <ul className="entry-list">
              {jobs.map((job) => (
                <JobCell
                  key={job.id}
                  job={job}
                  now={now}
                  timecodes={timecodes}
                  onTimecodesChange={setTimecodes}
                  onOpen={openTarget}
                />
              ))}
            </ul>
          </>
        )}
      </section>

      <TranscriptSearch onError={onError} />
    </>
  );
}
