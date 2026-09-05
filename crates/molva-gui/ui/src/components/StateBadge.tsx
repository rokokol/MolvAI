// SPDX-License-Identifier: MIT
// Бейдж состояния: цвет и текст совпадают с иконкой в трее.

import { useI18n } from "../i18n";
import type { DaemonState } from "../types";

/** Класс бейджа по состоянию: запись, работа, готовность, отсутствие связи. */
export function badgeClass(state: DaemonState | null, running: boolean): string {
  if (!running) {
    return "badge";
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

export default function StateBadge({
  state,
  running,
}: {
  state: DaemonState | null;
  running: boolean;
}) {
  const { t } = useI18n();
  const label = running ? t(`state.${state ?? "idle"}`) : t("state.offline");
  return (
    <span className={badgeClass(state, running)} role="status">
      <span className="dot" aria-hidden="true" />
      {label}
    </span>
  );
}
