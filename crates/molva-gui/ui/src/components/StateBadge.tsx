// SPDX-License-Identifier: MIT
// State badge: the color and the text match the tray icon

import { useI18n } from "../i18n";
import type { DaemonState, WorkerJob } from "../types";

/** Badge class by state: recording, processing, ready, no connection */
export function badgeClass(
  state: DaemonState | null,
  running: boolean,
  job: WorkerJob | null = null,
): string {
  if (!running) {
    return "badge";
  }
  // A file transcription occupies the recognizer while the state machine stays idle: the badge
  // must say busy, not ready
  if (job === "file" && (state === null || state === "idle")) {
    return "badge busy";
  }
  switch (state) {
    case "recording":
      return "badge recording";
    case "transcribing":
    case "post_processing":
    case "injecting":
      return "badge busy";
    default:
      return "badge ready";
  }
}

/** The text of a state with the job of the worker beside it */
export function stateLabel(
  state: DaemonState | null,
  running: boolean,
  job: WorkerJob | null,
  t: (key: string) => string,
): string {
  if (!running) {
    return t("state.offline");
  }
  if (job === "file" && (state === null || state === "idle")) {
    return t("state.file");
  }
  return t(`state.${state ?? "idle"}`);
}

export default function StateBadge({
  state,
  running,
  job = null,
}: {
  state: DaemonState | null;
  running: boolean;
  job?: WorkerJob | null;
}) {
  const { t } = useI18n();
  return (
    <span className={badgeClass(state, running, job)} role="status">
      {stateLabel(state, running, job, t)}
    </span>
  );
}
