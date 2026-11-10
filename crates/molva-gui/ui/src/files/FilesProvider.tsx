// SPDX-License-Identifier: MIT
// The queue of file transcriptions lives here, above the tabs.
//
// The view can be unmounted at any moment: leaving the Files tab, opening Settings, a reload of
// the window. The listeners and the jobs belong to the window, not to a view, so this provider
// owns them. The Files tab is a rendering of this state and holds no copy of it. Finished jobs
// are written to localStorage, so a restart shows them again; a job that was running at that
// moment comes back as interrupted, never as done.
//
// The work itself belongs to the daemon: the recognizer is one instance for dictation, the Files
// tab and the MCP server, so a cancelled job stops waiting here and cannot stop the daemon. A
// cancelled job never takes the answer that arrives later.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";

import { api, asCommandError } from "../api";
import type { CommandError, FileResult, TranscribeProgress } from "../types";
import {
  hydrateQueue,
  newJob,
  QUEUE_STORAGE_KEY,
  serializeQueue,
  type Job,
  type JobProgress,
  type SeekRequest,
} from "./jobs";

/** How long the queue waits after a change before it writes to localStorage */
const STORAGE_DEBOUNCE_MS = 300;
const CHUNKED_KEY = "molva.files.chunked";

/** The events the queue reacts to; the tests drive them without a window */
export type Action =
  | { type: "add"; jobs: Job[] }
  | { type: "start"; id: string; at: number }
  | { type: "progress"; id: string; progress: JobProgress }
  | { type: "done"; id: string; at: number; results: FileResult[] }
  | {
      type: "fail";
      id: string;
      at: number;
      status: "failed" | "cancelled";
      detail: string | null;
    }
  | { type: "requeue"; id: string }
  | { type: "remove"; id: string }
  | { type: "clear-finished" }
  | { type: "restore"; jobs: Job[] };

/**
 * The state of the queue after one event
 *
 * Exported because the races it settles are the behaviour: a progress event that lands after a
 * cancel, an answer that lands after a cancel, and a second start of the same job.
 */
export function reducer(state: Job[], action: Action): Job[] {
  switch (action.type) {
    case "add":
      return [...action.jobs, ...state];
    case "start":
      return state.map((job) =>
        job.id === action.id
          ? {
              ...job,
              status: "running",
              started_at: action.at,
              finished_at: null,
              progress: { phase: "starting", percent: null, elapsed_ms: 0 },
              detail: null,
            }
          : job,
      );
    case "progress":
      // A late event after a cancel must not restart a finished job
      return state.map((job) =>
        job.id === action.id && job.status === "running"
          ? { ...job, progress: action.progress }
          : job,
      );
    case "done":
      // A result that exists is the truth, even if a cancel raced it
      return state.map((job) =>
        job.id === action.id
          ? {
              ...job,
              status: "done",
              finished_at: action.at,
              progress: job.progress
                ? { ...job.progress, phase: "finalizing", percent: 100 }
                : null,
              results: action.results,
            }
          : job,
      );
    case "fail":
      return state.map((job) =>
        job.id === action.id && (job.status === "running" || job.status === "queued")
          ? {
              ...job,
              status: action.status,
              finished_at: action.at,
              detail: action.detail,
            }
          : job,
      );
    case "requeue":
      return state.map((job) =>
        job.id === action.id
          ? {
              ...job,
              status: "queued",
              started_at: null,
              finished_at: null,
              progress: null,
              detail: null,
              results: [],
            }
          : job,
      );
    case "remove":
      return state.filter((job) => job.id !== action.id);
    case "clear-finished":
      return state.filter(
        (job) => job.status === "queued" || job.status === "running",
      );
    case "restore":
      return action.jobs;
    default:
      return state;
  }
}

/** Read the stored queue once, before the first render */
function initialJobs(): Job[] {
  try {
    return hydrateQueue(localStorage.getItem(QUEUE_STORAGE_KEY));
  } catch {
    // A window that forbids storage still works; it just forgets the queue on exit
    return [];
  }
}

export interface FilesQueue {
  jobs: Job[];
  dragging: boolean;
  start: (path: string, chunked?: boolean) => void;
  startMany: (paths: string[], chunked?: boolean) => void;
  chunked: boolean;
  setChunked: (value: boolean) => void;
  cancel: (id: string) => void;
  retry: (id: string) => void;
  clearFinished: () => void;
  /** Drop one row of the queue: the journal keeps what it has */
  remove: (id: string) => void;
  isExpanded: (id: string) => boolean;
  toggleExpanded: (id: string) => void;
  seekFor: (jobId: string, resultIndex: number) => SeekRequest | null;
  /** Move the player of one result to a timecode; one place owns every seek */
  seek: (jobId: string, resultIndex: number, ms: number) => void;
  /** Expand the job that holds this file and move its player to the timecode */
  reveal: (path: string, ms: number | null) => boolean;
  /** Whether a finished job of this window holds the file */
  holds: (path: string) => boolean;
}

const FilesQueueContext = createContext<FilesQueue | null>(null);

export function useFilesQueue(): FilesQueue {
  const value = useContext(FilesQueueContext);
  if (!value) {
    throw new Error("useFilesQueue is used outside FilesProvider");
  }
  return value;
}

let counter = 0;

function makeId(): string {
  counter += 1;
  return `job-${Date.now()}-${counter}`;
}

export function FilesProvider({
  onError,
  children,
}: {
  onError: (error: CommandError | null) => void;
  children: ReactNode;
}) {
  const [jobs, dispatch] = useReducer(reducer, undefined, initialJobs);
  const [dragging, setDragging] = useState(false);
  const [chunked, setChunkedState] = useState(() => {
    try {
      // Cutting at the pauses is the better default: the model sees whole phrases instead of one
      // long recording, and the timecodes of the result follow those pauses. A stored choice wins
      return localStorage.getItem(CHUNKED_KEY) !== "0";
    } catch {
      return true;
    }
  });
  const setChunked = useCallback((value: boolean) => {
    setChunkedState(value);
    try {
      localStorage.setItem(CHUNKED_KEY, value ? "1" : "0");
    } catch {
      // The choice does not persist, but the toggle works for this session
    }
  }, []);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [seeks, setSeeks] = useState<Record<string, SeekRequest>>({});
  const seekSeq = useRef(0);
  // A job whose command was started once: React runs an effect twice in development, and a
  // second request for the same job would be a duplicate, not a retry
  const started = useRef<Set<string>>(new Set());
  // Jobs the user stopped waiting for: the daemon keeps transcribing, and its answer must not
  // revive a job that the window already shows as cancelled
  const cancelled = useRef<Set<string>>(new Set());

  const run = useCallback(
    async (job: Job) => {
      try {
        const results = await api.transcribeFile(job.id, job.path, job.chunked);
        if (cancelled.current.has(job.id)) {
          return;
        }
        dispatch({ type: "done", id: job.id, at: Date.now(), results });
      } catch (err) {
        if (cancelled.current.has(job.id)) {
          return;
        }
        const error = asCommandError(err);
        dispatch({
          type: "fail",
          id: job.id,
          at: Date.now(),
          status: "failed",
          detail: error.message,
        });
        onError(error);
      }
    },
    [onError],
  );

  // One file at a time: the daemon transcribes one file at a time and never beside a phrase,
  // so a burst of drops would only wait in the window instead of in the daemon
  useEffect(() => {
    if (jobs.some((job) => job.status === "running")) {
      return;
    }
    const next = [...jobs].reverse().find((job) => job.status === "queued");
    if (!next || started.current.has(next.id)) {
      return;
    }
    started.current.add(next.id);
    dispatch({ type: "start", id: next.id, at: Date.now() });
    void run(next);
  }, [jobs, run]);

  useEffect(() => {
    const progress = listen<TranscribeProgress>(
      "molva://transcribe-progress",
      (event) => {
        const payload = event.payload;
        dispatch({
          type: "progress",
          id: payload.id,
          progress: {
            phase: payload.phase,
            percent: payload.percent,
            elapsed_ms: payload.elapsed_ms,
          },
        });
      },
    );
    // The webview itself handles file drops: the paths arrive ready
    const dropped = listen<{ paths: string[] }>("tauri://drag-drop", (event) => {
      setDragging(false);
      const paths = event.payload.paths.filter((path) => path.trim() !== "");
      if (paths.length > 0) {
        dispatch({
          type: "add",
          jobs: paths.map((path) => newJob(makeId(), path, chunked)),
        });
      }
    });
    const hovered = listen("tauri://drag-enter", () => setDragging(true));
    const left = listen("tauri://drag-leave", () => setDragging(false));
    return () => {
      void Promise.all([progress, dropped, hovered, left]).then((fns) =>
        fns.forEach((fn) => fn()),
      );
    };
  }, [chunked]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      try {
        localStorage.setItem(QUEUE_STORAGE_KEY, serializeQueue(jobs));
      } catch {
        // A full or forbidden storage costs the queue on the next launch, not this session
      }
    }, STORAGE_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [jobs]);

  const startMany = useCallback((paths: string[], chunked = false) => {
    const cleaned = paths.map((path) => path.trim()).filter((path) => path !== "");
    if (cleaned.length === 0) {
      return;
    }
    dispatch({
      type: "add",
      jobs: cleaned.map((path) => newJob(makeId(), path, chunked)),
    });
  }, []);

  const start = useCallback(
    (path: string, chunked = false) => startMany([path], chunked),
    [startMany],
  );

  const cancel = useCallback(
    (id: string) => {
      const job = jobs.find((item) => item.id === id);
      if (!job) {
        return;
      }
      if (job.status === "queued") {
        dispatch({
          type: "fail",
          id,
          at: Date.now(),
          status: "cancelled",
          detail: null,
        });
        return;
      }
      if (job.status !== "running") {
        return;
      }
      // The daemon owns the recognizer, so the window asks it to stop: the pipeline reads the
      // flag between two chunks, and the answer of the stopped job reaches nobody. The window
      // stops waiting at once, so the row is free even while the model finishes its chunk
      cancelled.current.add(id);
      void api.fileCancel().catch(() => undefined);
      dispatch({
        type: "fail",
        id,
        at: Date.now(),
        status: "cancelled",
        detail: null,
      });
    },
    [jobs],
  );

  const retry = useCallback((id: string) => {
    started.current.delete(id);
    cancelled.current.delete(id);
    dispatch({ type: "requeue", id });
  }, []);

  const clearFinished = useCallback(() => {
    dispatch({ type: "clear-finished" });
  }, []);

  const remove = useCallback((id: string) => {
    cancelled.current.delete(id);
    dispatch({ type: "remove", id });
  }, []);

  const toggleExpanded = useCallback((id: string) => {
    setExpanded((prev) => ({ ...prev, [id]: !prev[id] }));
  }, []);

  const isExpanded = useCallback((id: string) => Boolean(expanded[id]), [expanded]);

  const holdIndex = useCallback(
    (path: string): { jobId: string; resultIndex: number } | null => {
      for (const job of jobs) {
        const resultIndex = job.results.findIndex((result) => result.file === path);
        if (resultIndex >= 0) {
          return { jobId: job.id, resultIndex };
        }
      }
      return null;
    },
    [jobs],
  );

  const reveal = useCallback(
    (path: string, ms: number | null): boolean => {
      const hold = holdIndex(path);
      if (!hold) {
        return false;
      }
      setExpanded((prev) => ({ ...prev, [hold.jobId]: true }));
      if (ms !== null) {
        seekSeq.current += 1;
        const key = `${hold.jobId}:${hold.resultIndex}`;
        setSeeks((prev) => ({ ...prev, [key]: { ms, seq: seekSeq.current } }));
      }
      return true;
    },
    [holdIndex],
  );
  const holds = useCallback((path: string) => holdIndex(path) !== null, [holdIndex]);

  const seek = useCallback((jobId: string, resultIndex: number, ms: number) => {
    seekSeq.current += 1;
    const key = `${jobId}:${resultIndex}`;
    setSeeks((prev) => ({ ...prev, [key]: { ms, seq: seekSeq.current } }));
  }, []);

  const seekFor = useCallback(
    (jobId: string, resultIndex: number) => seeks[`${jobId}:${resultIndex}`] ?? null,
    [seeks],
  );

  const value = useMemo<FilesQueue>(
    () => ({
      jobs,
      dragging,
      start,
      startMany,
      chunked,
      setChunked,
      cancel,
      retry,
      clearFinished,
      remove,
      isExpanded,
      toggleExpanded,
      seekFor,
      seek,
      reveal,
      holds,
    }),
    [
      jobs,
      dragging,
      start,
      startMany,
      // The switch belongs to the value: without these two the row keeps the state of the first
      // render, and a press on it changes nothing the view can see
      chunked,
      setChunked,
      cancel,
      retry,
      clearFinished,
      remove,
      isExpanded,
      toggleExpanded,
      seekFor,
      seek,
      reveal,
      holds,
    ],
  );

  return (
    <FilesQueueContext.Provider value={value}>{children}</FilesQueueContext.Provider>
  );
}
