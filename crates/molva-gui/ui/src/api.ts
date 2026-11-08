// SPDX-License-Identifier: MIT
// Wrappers over the Rust commands. All arithmetic and validation live in the core.
// Here we only have calls and the conversion of an error to a common form

import { invoke } from "@tauri-apps/api/core";

import type {
  Config,
  CommandError,
  DeviceInfo,
  Entry,
  FileResult,
  HyprlandBinds,
  LlmProbeReport,
  LlmProviderOption,
  MediaInfo,
  PathSuggestion,
  ProviderKeyStatus,
  SearchHit,
  StatsSummary,
  Status,
  SttModelOption,
  SttProbeReport,
  StyleOption,
} from "./types";

/** A command error always arrives as an object; a string means the bridge itself failed */
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
  builtinStyles: () => invoke<StyleOption[]>("builtin_styles"),
  /** The catalog of `models.toml`, each entry with the engine that can load it */
  sttModels: () => invoke<SttModelOption[]>("stt_models"),
  /** Whether this build can recognize through a remote API */
  sttRemoteAvailable: () => invoke<boolean>("stt_remote_available"),
  /** Check the draft recognition settings: no recording, no microphone */
  testStt: (config: Config) => invoke<SttProbeReport>("test_stt", { config }),
  /** Send one short prompt through the draft model settings; nothing is saved */
  testLlm: (config: Config) => invoke<LlmProbeReport>("test_llm", { config }),
  llmProviders: () => invoke<LlmProviderOption[]>("llm_providers"),
  /** Whether the store holds a key entry; the value never comes back */
  providerKeyStatus: (provider: string, source: string) =>
    invoke<ProviderKeyStatus>("provider_key_status", { provider, source }),
  storeProviderKey: (provider: string, source: string, key: string) =>
    invoke<ProviderKeyStatus>("store_provider_key", { provider, source, key }),
  removeProviderKey: (provider: string, source: string) =>
    invoke<ProviderKeyStatus>("remove_provider_key", { provider, source }),
  /** Validate a captured combination with the parser the daemon uses */
  checkHotkey: (value: string) => invoke<{ value: string }>("check_hotkey", { value }),
  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  injectText: (text: string, mode?: string) =>
    invoke<void>("inject_text", { text, mode }),
  reloadConfig: () => invoke<void>("reload_config"),
  startDaemon: () => invoke<void>("start_daemon"),
  stopDaemon: () => invoke<void>("stop_daemon"),

  getConfig: () => invoke<Config>("get_config"),
  /** The config is a read-only file from the Nix store, so the settings page is hidden */
  configManaged: () => invoke<boolean>("config_managed"),
  /** The resolved interface language; the Rust side resolves it so the tray agrees */
  uiLanguage: () => invoke<string>("ui_language"),
  getConfigPath: () => invoke<string>("get_config_path"),
  isFirstRun: () => invoke<boolean>("is_first_run"),
  saveConfig: (config: Config) => invoke<void>("save_config", { config }),
  exportConfig: () => invoke<string>("export_config"),
  importConfig: (path: string) => invoke<Config>("import_config", { path }),
  resetConfig: () => invoke<Config>("reset_config"),
  hyprlandBinds: () => invoke<HyprlandBinds>("hyprland_binds"),
  writeHyprlandBinds: () => invoke<HyprlandBinds>("write_hyprland_binds"),
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
    invoke<FileResult[]>("transcribe_file", { id, path }),

  /** Widen the asset protocol scope for one file and get the URL that plays it */
  prepareMedia: (path: string) => invoke<MediaInfo>("prepare_media", { path }),

  /** Entries that complete the typed path; never throws on an unreadable directory */
  pathSuggestions: (input: string, limit?: number) =>
    invoke<PathSuggestion[]>("path_suggestions", { input, limit: limit ?? null }),

  /** Search the transcriptions of audio and video files, newest first */
  searchFiles: (query: string, limit: number) =>
    invoke<SearchHit[]>("search_files", { query, limit }),
};

/**
 * Copy to the clipboard without a plugin: first the system API, then a fallback
 * method with a hidden field. A webview without a secure context has no system API
 */
export async function copyToClipboard(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Go on to the fallback method
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
