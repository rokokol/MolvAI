// SPDX-License-Identifier: MIT
// One cell of the queue: the name, the state and a progress bar while collapsed; the player,
// the loudness of the recording and the timecoded transcript once expanded.

import { useCallback, useState } from "react";

import { api } from "../api";
import { formatElapsed } from "../format";
import { useI18n } from "../i18n";
import type { FileResult } from "../types";
import { useFilesQueue } from "./FilesProvider";
import {
  elapsedMs,
  phaseKey,
  statusKey,
  statusTone,
  type Job,
  type SeekRequest,
} from "./jobs";
import MediaPreview from "./MediaPreview";
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
  const { isExpanded, toggleExpanded, cancel, retry, seekFor, remove } = useFilesQueue();
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
          {!running && job.status !== "queued" && (
            <button
              type="button"
              className="chip danger"
              title={t("files.removeHint")}
              onClick={() => remove(job.id)}
            >
              {t("files.remove")}
            </button>
          )}
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
              <ResultPane
                key={`${job.id}-${index}`}
                jobId={job.id}
                index={index}
                result={result}
                seekRequest={seekFor(job.id, index)}
                timecodes={timecodes}
                onTimecodesChange={onTimecodesChange}
                onOpen={onOpen}
              />
            ))
          )}
        </div>
      )}
    </li>
  );
}

/**
 * One result of a job: the player and its transcript.
 *
 * The position of the player is the state of this result alone, and the callbacks into the
 * transcript keep their identity, so a tick of the player rebuilds neither the other results of
 * the job nor the rows of the transcript that did not move
 */
function ResultPane({
  jobId,
  index,
  result,
  seekRequest,
  timecodes,
  onTimecodesChange,
  onOpen,
}: {
  jobId: string;
  index: number;
  result: FileResult;
  seekRequest: SeekRequest | null;
  timecodes: boolean;
  onTimecodesChange?: (value: boolean) => void;
  onOpen: (path: string) => void;
}) {
  const { seek } = useFilesQueue();
  const [position, setPosition] = useState<number | null>(null);
  const onTime = useCallback((ms: number) => setPosition(ms), []);
  const onSeek = useCallback(
    (ms: number) => seek(jobId, index, ms),
    [seek, jobId, index],
  );
  return (
    <section className="result">
      {/* The player keeps its size and the timecoded transcript scrolls beside it, so
          a long lecture does not push the rest of the queue off the screen */}
      <div className="result-media">
        <MediaPreview
          path={result.file}
          seek={seekRequest}
          onTime={onTime}
          onOpen={onOpen}
          prepare={api.prepareMedia}
        />
      </div>
      <div className="result-text">
        <Transcript
          result={result}
          timecodes={timecodes}
          currentMs={position}
          onSeek={onSeek}
          onTimecodesChange={onTimecodesChange}
          onOpen={onOpen}
        />
      </div>
    </section>
  );
}
