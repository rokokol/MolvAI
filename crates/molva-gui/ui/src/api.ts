// SPDX-License-Identifier: MIT
// Обёртки над командами Rust. Вся арифметика и валидация живут в ядре,
// здесь только вызовы и приведение ошибки к общему виду.

import { invoke } from "@tauri-apps/api/core";

import type {
  Config,
  CommandError,
  DeviceInfo,
  Entry,
  StatsSummary,
  Status,
  StyleOption,
} from "./types";

/** Ошибка команды всегда приходит объектом; строка — признак сбоя самого моста. */
export function asCommandError(error: unknown): CommandError {
  if (error && typeof error === "object" && "message" in error) {
    return error as CommandError;
  }
  return { kind: "internal", message: String(error) };
}

export interface HistoryFilter {
  query: string;
  app: string;
  since: string | null;
  until: string | null;
  limit: number;
}

export const api = {
  getStatus: () => invoke<Status>("get_status"),
  recordStart: (mode?: string, style?: string) =>
    invoke<void>("record_start", { mode, style }),
  recordStop: () => invoke<void>("record_stop"),
  recordToggle: (mode?: string) => invoke<void>("record_toggle", { mode }),
  recordCancel: () => invoke<void>("record_cancel"),
  setStyle: (style: string) => invoke<void>("set_style", { style }),
  availableStyles: () => invoke<StyleOption[]>("available_styles"),
  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  injectText: (text: string, mode?: string) =>
    invoke<void>("inject_text", { text, mode }),
  reloadConfig: () => invoke<void>("reload_config"),
  startDaemon: () => invoke<void>("start_daemon"),
  stopDaemon: () => invoke<void>("stop_daemon"),

  getConfig: () => invoke<Config>("get_config"),
  getConfigPath: () => invoke<string>("get_config_path"),
  isFirstRun: () => invoke<boolean>("is_first_run"),
  saveConfig: (config: Config) => invoke<void>("save_config", { config }),
  exportConfig: () => invoke<string>("export_config"),
  importConfig: (path: string) => invoke<Config>("import_config", { path }),
  resetConfig: () => invoke<Config>("reset_config"),
  hyprlandSnippet: () => invoke<string>("hyprland_snippet"),
  setAutostart: (enabled: boolean) => invoke<boolean>("set_autostart", { enabled }),
  getAutostart: () => invoke<boolean>("get_autostart"),
  toggleHotkeysPaused: () => invoke<boolean>("toggle_hotkeys_paused"),

  historyList: (filter: Partial<HistoryFilter>) =>
    invoke<Entry[]>("history_list", {
      filter: {
        query: filter.query ?? "",
        app: filter.app ?? "",
        since: filter.since ?? null,
        until: filter.until ?? null,
        limit: filter.limit ?? 0,
      },
    }),
  historyApps: () => invoke<string[]>("history_apps"),
  historyDelete: (id: string) => invoke<boolean>("history_delete", { id }),
  historyClear: () => invoke<void>("history_clear"),
  dataDirPath: () => invoke<string>("data_dir_path"),
  openPath: (path: string) => invoke<void>("open_path", { path }),

  statsSummary: (rangeDays: number) =>
    invoke<StatsSummary>("stats_summary", { rangeDays }),
  statsExportCsv: (rangeDays: number) =>
    invoke<string>("stats_export_csv", { rangeDays }),

  transcribeFile: (id: string, path: string) =>
    invoke<unknown>("transcribe_file", { id, path }),
  transcribeCancel: (id: string) => invoke<boolean>("transcribe_cancel", { id }),
};

/**
 * Копирование в буфер обмена без плагина: сначала системный API, затем запасной
 * приём со скрытым полем — в webview без защищённого контекста первый недоступен.
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Переходим к запасному приёму.
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.opacity = "0";
  document.body.appendChild(area);
  area.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }
  document.body.removeChild(area);
  return copied;
}
