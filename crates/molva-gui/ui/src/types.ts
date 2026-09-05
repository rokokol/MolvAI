// SPDX-License-Identifier: MIT
// Формы данных, приходящих из Rust. Имена полей повторяют serde-представление
// molva_core и команд GUI: расхождение здесь — это сразу ошибка типов при сборке.

export type DaemonState =
  | "idle"
  | "recording"
  | "transcribing"
  | "post_processing"
  | "injecting";

export type Mode = "dictation" | "command";

export interface LatencyMs {
  stt: number;
  rules: number;
  llm?: number | null;
  inject?: number | null;
  total: number;
  first_hypothesis?: number | null;
  stop_after_release?: number | null;
}

export interface Tokens {
  prompt: number;
  completion: number;
}

/** Строка журнала: одна реплика. */
export interface Entry {
  schema: number;
  id: string;
  ts: string;
  session_id: string;
  mode: Mode;
  source: "mic" | "file";
  app?: string | null;
  language?: string | null;
  audio_secs: number;
  words: number;
  wpm?: number | null;
  style: string;
  stt_engine: string;
  stt_model: string;
  llm_provider?: string | null;
  llm_model?: string | null;
  llm_used: boolean;
  local_llm: boolean;
  dict_hits: number;
  inject_method?: string | null;
  latency_ms: LatencyMs;
  tokens?: Tokens | null;
  error?: string | null;
  text_raw?: string | null;
  text_final?: string | null;
  audio_path?: string | null;
}

export interface DeviceInfo {
  name: string;
  is_default: boolean;
  sample_rates: number[];
}

export interface Status {
  daemon_running: boolean;
  daemon_ours: boolean;
  state: DaemonState | null;
  style: string | null;
  hotkeys_paused: boolean;
  message?: string;
  hint?: string;
}

/** Ошибка команды: вид, сообщение, следующий шаг и поле формы. */
export interface CommandError {
  kind: string;
  message: string;
  hint?: string;
  field?: string;
}

export interface StyleOption {
  id: string;
  name: string;
}

// --- Статистика: форма зафиксирована контрактом с дорожкой D ---

export interface StatsLatency {
  stt: number;
  llm?: number | null;
  inject?: number | null;
  total: number;
}

export interface DayPoint {
  day: string;
  entries: number;
  words: number;
  audio_secs: number;
  avg_wpm?: number | null;
  avg_latency_ms: number;
}

export interface AppRow {
  app: string;
  entries: number;
  words: number;
  avg_wpm?: number | null;
}

export interface StatsSummary {
  total_words: number;
  words_today: number;
  avg_wpm_today: number | null;
  avg_wpm_7d: number | null;
  avg_wpm_all: number | null;
  record_wpm: number | null;
  record_at: string | null;
  streak_days: number;
  minutes_recorded: number;
  saved_minutes: number;
  latency_ms: StatsLatency;
  tokens: Tokens;
  series: DayPoint[];
  by_app: AppRow[];
}

// --- Настройки: зеркало molva_core::Config ---

export interface RemoteSttConfig {
  base_url: string;
  api_key_source: string;
  api_key_env: string;
  model: string;
}

export interface Config {
  version: number;
  ui_language: string;
  audio: {
    device: string;
    gain: number;
    max_duration_secs: number;
    trim_silence: boolean;
    silence_threshold_db: number;
    vad_min_pause_ms: number;
    noise_suppression: boolean;
    sounds: boolean;
    sound_volume: number;
    warn_zero_level: boolean;
  };
  stt: {
    engine: string;
    model: string;
    model_path: string;
    language: string;
    allowed_languages: string[];
    threads: number;
    unload_after_secs: number;
    no_speech_threshold: number;
    streaming_preview: boolean;
    remote: RemoteSttConfig;
  };
  dictionary: { path: string; fuzzy: boolean; in_prompt: boolean };
  rules: {
    enabled: boolean;
    spoken_punctuation: boolean;
    auto_punctuation: boolean;
    remove_fillers: boolean;
    remove_repeats: boolean;
    numbers_as_digits: boolean;
    paragraph_pause_ms: number;
    llm_min_words: number;
  };
  llm: {
    enabled: boolean;
    provider: string;
    base_url: string;
    model: string;
    api_key_source: string;
    api_key_env: string;
    temperature: number;
    max_tokens: number;
    timeout_secs: number;
    max_retries: number;
  };
  style: {
    default: string;
    by_app: Record<string, string>;
    custom: { id: string; name: string; uses_llm: boolean; system_prompt: string }[];
  };
  output: {
    mode: string;
    auto_type_max_chars: number;
    restore_clipboard: boolean;
    restore_delay_ms: number;
    paste_backend: string;
    type_backend: string;
    type_delay_ms: number;
    terminal_shortcut: boolean;
    notify_on_fallback: boolean;
  };
  hotkeys: {
    backend: string;
    push_to_talk: string;
    toggle: string;
    command: string;
    cancel: string;
    style_next: string;
    tap_toggles: boolean;
    short_press_ms: number;
    min_hold_ms: number;
    double_tap_ms: number;
  };
  command_mode: { enabled: boolean; system_prompt: string };
  journal: {
    path: string;
    enabled: boolean;
    include_text: boolean;
    keep_audio: boolean;
    max_entries: number;
    max_size_mb: number;
  };
  stats: { typing_baseline_wpm: number };
  privacy: { send_to_llm: boolean; no_record_mode: boolean; telemetry: boolean };
  autostart: { enabled: boolean };
  log: { level: string; max_size_mb: number };
}

/** Присутствие демона: событие `molva://daemon`. */
export interface DaemonPresence {
  connected: boolean;
  message?: string;
  hint?: string;
}

export interface TranscribeProgress {
  id: string;
  line: string;
}
