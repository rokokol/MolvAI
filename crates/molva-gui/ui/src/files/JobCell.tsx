// SPDX-License-Identifier: MIT
// One cell of the queue: the name, the state and a progress bar while collapsed; the player,
// the spectrogram and the timecoded transcript once expanded.

import { formatElapsed } from "../format";
import { useI18n } from "../i18n";
import { useFilesQueue } from "./FilesProvider";
import {
  elapsedMs,
  phaseKey,
  statusKey,
  statusTone,
  type Job,
} from "./jobs";
import ProgressBar from "./ProgressBar";
import Transcript from "./Transcript";

export interface JobCellProps {
  job: Job;
  /** The clock, ticking while something runs; it keeps the elapsed time alive across tabs */
  now: number;
  timecodes: boolean;
  onTimecodesChange?: (value: boolean) => void;
  onOpen: (path: string) => void;
}

export default function JobCell({
  job,
  now,
  timecodes,
  onTimecodesChange,
  onOpen,
}: JobCellProps) {
  const { t } = useI18n();
  const { isExpanded, toggleExpanded, cancel, retry } = useFilesQueue();
  const expanded = isExpanded(job.id);
  const status = t(statusKey(job.status));
  const elapsed = elapsedMs(job, now);
  const running = job.status === "running";
  const phase = running && job.progress ? t(phaseKey(job.progress.phase)) : null;
  const resultCount = job.results.length;

  return (
    <li className={`entry job ${job.status}`}>
      <div className="job-head">
        <button
          type="button"
          className="job-expander"
          aria-expanded={expanded}
          aria-controls={`job-body-${job.id}`}
          title={job.path}
          onClick={() => toggleExpanded(job.id)}
        >
          <span className="job-chevron" aria-hidden="true">
            {expanded ? "▾" : "▸"}
          </span>
          <span className="job-name mono">{job.name}</span>
        </button>
        <span className={`badge ${statusTone(job.status)}`}>{status}</span>
        {elapsed !== null && (
          <span className="small muted mono" title={t("files.elapsed")}>
            {formatElapsed(elapsed)}
          </span>
        )}
        {job.status === "done" && (
          <span className="small muted">
            {t("files.resultCount", { count: resultCount })}
          </span>
        )}
        <div className="row job-actions">
          {job.status === "queued" && (
            <button type="button" className="chip" onClick={() => cancel(job.id)}>
              {t("files.cancel")}
            </button>
          )}
          {running && (
            <button
              type="button"
              className="chip"
              title={t("files.stopWaitingHint")}
              onClick={() => cancel(job.id)}
            >
              {t("files.stopWaiting")}
            </button>
          )}
          {(job.status === "failed" ||
            job.status === "cancelled" ||
            job.status === "interrupted") && (
            <button type="button" className="chip" onClick={() => retry(job.id)}>
              {t("files.retry")}
            </button>
          )}
          <button type="button" className="chip" onClick={() => onOpen(job.path)}>
            {t("files.open")}
          </button>
        </div>
      </div>

      <p className="mono small muted job-path">{job.path}</p>

      {running && (
        <div className="job-progress">
          <ProgressBar
            percent={job.progress?.percent ?? null}
            label={phase ?? status}
          />
          <div className="row spread small muted">
            <span>{phase}</span>
            <span className="mono">{formatElapsed(elapsed)}</span>
          </div>
          <p className="small muted">{t("files.progressUnknown")}</p>
        </div>
      )}
      {job.status === "done" && (
        <div className="job-progress">
          <ProgressBar percent={100} label={status} tone="done" />
        </div>
      )}
      {job.status === "interrupted" && (
        <p className="small muted">{t("files.interruptedHint")}</p>
      )}
      {job.detail && job.status === "failed" && (
        <p className="small mono job-detail">{job.detail}</p>
      )}

      {expanded && (
        <div className="job-body" id={`job-body-${job.id}`}>
          {resultCount === 0 ? (
            <p className="small muted">
              {running
                ? t("files.waitingResult")
                : job.status === "done"
                  ? t("files.emptyResult")
                  : t("files.noResult")}
            </p>
          ) : (
            job.results.map((result, index) => (
              <section className="result" key={`${job.id}-${index}`}>
                <p className="small muted">{t("files.previewDisabled")}</p>
                <Transcript
                  result={result}
                  timecodes={timecodes}
                  onTimecodesChange={onTimecodesChange}
                  onOpen={onOpen}
                />
              </section>
            ))
          )}
        </div>
      )}
    </li>
  );
}
