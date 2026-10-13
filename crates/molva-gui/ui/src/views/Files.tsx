// SPDX-License-Identifier: MIT
// Audio file transcription: drag into the window or type a path, with progress and cancel

import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { api, asCommandError } from "../api";
import { useI18n } from "../i18n";
import type { ViewProps } from "../App";
import type { TranscribeProgress } from "../types";

type JobStatus = "running" | "done" | "failed" | "cancelled";

interface Job {
  id: string;
  path: string;
  status: JobStatus;
  /** The last progress line from the CLI stderr */
  progress: string;
  result?: string;
}

/** Job identifier: the clock plus a counter, so it does not depend on crypto in the webview */
function nextId(counter: number): string {
  return `job-${Date.now()}-${counter}`;
}

export default function Files({ onError }: ViewProps) {
  const { t } = useI18n();
  const [jobs, setJobs] = useState<Job[]>([]);
  const [path, setPath] = useState("");
  const [over, setOver] = useState(false);
  const counter = useRef(0);

  const update = (id: string, patch: Partial<Job>) => {
    setJobs((prev) => prev.map((job) => (job.id === id ? { ...job, ...patch } : job)));
  };

  const start = (target: string) => {
    const trimmed = target.trim();
    if (!trimmed) {
      return;
    }
    counter.current += 1;
    const id = nextId(counter.current);
    setJobs((prev) => [{ id, path: trimmed, status: "running", progress: "" }, ...prev]);
    api
      .transcribeFile(id, trimmed)
      .then((result) => {
        update(id, { status: "done", result: JSON.stringify(result) });
      })
      .catch((err) => {
        const error = asCommandError(err);
        const cancelled = error.kind === "cancelled";
        update(id, {
          status: cancelled ? "cancelled" : "failed",
          progress: error.message,
        });
        if (!cancelled) {
          onError(error);
        }
      });
  };

  useEffect(() => {
    const progress = listen<TranscribeProgress>("molva://transcribe", (event) => {
      update(event.payload.id, { progress: event.payload.line });
    });
    // The webview itself handles file drops: the paths arrive ready
    const dropped = listen<{ paths: string[] }>("tauri://drag-drop", (event) => {
      setOver(false);
      event.payload.paths.forEach(start);
    });
    const hovered = listen("tauri://drag-enter", () => setOver(true));
    const left = listen("tauri://drag-leave", () => setOver(false));
    return () => {
      void Promise.all([progress, dropped, hovered, left]).then((fns) =>
        fns.forEach((fn) => fn()),
      );
    };
    // The handlers are set once for the whole life of the tab: they depend
    // only on setJobs and a ref with the counter, and these do not change between renders
  }, []);

  const statusLabel = (status: JobStatus) =>
    ({
      running: t("files.statusRunning"),
      done: t("files.statusDone"),
      failed: t("files.statusFailed"),
      cancelled: t("files.statusCancelled"),
    })[status];

  return (
    <>
      <h1>{t("files.title")}</h1>

      <section className="card">
        <div className={over ? "dropzone over" : "dropzone"}>{t("files.drop")}</div>
        <div className="row" style={{ marginTop: "0.75rem", alignItems: "flex-end" }}>
          <div className="field" style={{ marginBottom: 0, flex: "1 1 18rem" }}>
            <label htmlFor="files-path">{t("files.pathLabel")}</label>
            <input
              id="files-path"
              type="text"
              value={path}
              onChange={(event) => setPath(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  start(path);
                  setPath("");
                }
              }}
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
        <p className="small muted">{t("files.hint")}</p>
      </section>

      <section className="card">
        {jobs.length === 0 ? (
          <p className="muted">{t("files.queueEmpty")}</p>
        ) : (
          <ul className="entry-list">
            {jobs.map((job) => (
              <li key={job.id} className="entry">
                <div className="meta">
                  <span>{statusLabel(job.status)}</span>
                  <span className="mono">{job.path}</span>
                </div>
                {job.progress && <p className="small mono">{job.progress}</p>}
                {job.result && <p className="small mono">{job.result}</p>}
                {job.status === "running" && (
                  <button
                    type="button"
                    className="chip"
                    onClick={() => {
                      void api.transcribeCancel(job.id);
                      update(job.id, { status: "cancelled" });
                    }}
                  >
                    {t("files.cancel")}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </>
  );
}
