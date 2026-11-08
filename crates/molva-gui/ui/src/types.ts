// SPDX-License-Identifier: MIT
// Shapes of the data that comes from Rust. The field names repeat the serde form
// of molva_core and of the GUI commands: a mismatch here is a type error at build time

export type DaemonState =
  | "idle"
  | "recording"
  | "transcribing"
  | "post_processing"
  | "injecting";

/** What the worker thread does beside the state machine: a file holds the recognizer while the
 * state stays `idle`, and the window must say so instead of showing "Ready" */
export type WorkerJob = "file";

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

/** A journal line: one phrase */
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

/** A fragment of a file transcription with its place in the recording */
export interface Segment {
  start_ms: number;
  end_ms: number;
  text: string;
}

/** One result of `molva transcribe --json` */
export interface FileResult {
  file: string;
  text: string;
  segments?: Segment[];
  audio_secs: number;
  latency_ms: number;
  language?: string | null;
}

/** One match of the transcript search, shaped by the Rust side */
export interface SearchHit {
  entry_id: string;
  file: string;
  recorded_at: string;
  text: string;
  start_ms?: number | null;
  end_ms?: number | null;
  timecode?: string | null;
  language?: string | null;
}

export interface Status {
  daemon_running: boolean;
  daemon_ours: boolean;
  state: DaemonState | null;
  /** A file transcription holds the recognizer while `state` stays `idle` */
  job: WorkerJob | null;
  style: string | null;
  hotkeys_paused: boolean;
  message?: string;
  hint?: string;
  /** The daemon has not opened the socket yet, because it downloads model weights */
  download?: DownloadProgress;
  /** The daemon that the window started has crashed: exit code and its last stderr line */
  exit_reason?: string;
}

/** Model weights download: bytes of the `.part` file and the size from the catalog */
export interface DownloadProgress {
  model: string;
  downloaded: number;
  total: number;
}

/** A command error: kind, message, next step and form field */
export interface CommandError {
  kind: string;
  message: string;
  hint?: string;
  field?: string;
}

/** The Hyprland binds file that MolvAI writes: its path, the include line, whether it exists */
export interface HyprlandBinds {
  path: string;
  include_line: string;
  written: boolean;
  /** The window runs inside a Hyprland session */
  session: boolean;
}

export interface StyleOption {
  id: string;
  name: string;
}

/** One set of weights from `models.toml`, with the engine that can load it */
export interface SttModelOption {
  name: string;
  engine: string;
  language?: string | null;
  size_bytes: number;
  installed: boolean;
}

/** A post-processing provider with the URL and the local flag that the core client uses */
export interface LlmProviderOption {
  id: string;
  base_url: string;
  model: string;
  api_key_env: string;
  is_local: boolean;
}

/** An entry of the secret store: whether it exists, never the value */
export interface ProviderKeyStatus {
  present: boolean;
  entry: string;
  detail?: string;
}

/** What the model test of the settings page found */
export interface LlmProbeReport {
  /** The provider that answered: `ollama`, `openrouter`… */
  provider: string;
  model: string;
  is_local: boolean;
  /** From the first request to the answer, retries included */
  latency_ms: number;
  /** Requests sent: above one means the provider failed and then answered */
  attempts: number;
  answer: string;
}

/** One line of the recognition test */
export interface SttProbeCheck {
  /** `engine` | `remote` | `model` | `cli`, so the page translates the title */
  id: string;
  ok: boolean;
  /** An error text, or the path the check used; empty when a passing check needs no detail */
  detail: string;
}

/** What the recognition test of the settings page found */
export interface SttProbeReport {
  engine: string;
  model: string;
  ok: boolean;
  checks: SttProbeCheck[];
}

// --- Statistics: the contract with track D fixes the shape ---

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

// --- Settings: a mirror of molva_core::Config ---
//
// The mirror must be complete: `save_config` deserializes this object with `serde(default)`,
// so a field missing here is silently reset to its default on every save

export interface CustomStyle {
  id: string;
  name: string;
  uses_llm: boolean;
  system_prompt: string;
}

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
    models_mirror: string;
    language: string;
    allowed_languages: string[];
    threads: number;
    unload_after_secs: number;
    no_speech_threshold: number;
    auto_pull: boolean;
    chunked: boolean;
    chunk_pause_ms: number;
    streaming_preview: boolean;
    streaming_inject: boolean;
    transcribe_cli: string;
    transcribe_backend: string;
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
    /** `keyring` | `file` | `env` | `plain` | `none` */
    api_key_source: string;
    api_key_env: string;
    /** The key itself, used only by `plain`; the settings file keeps it in clear text */
    api_key_plain: string;
    /** OpenRouter data collection: `allow` or `deny` */
    data_collection: string;
    zdr: boolean;
    /** OpenRouter hosters in the order to try them, comma-separated; empty means the router picks */
    provider_order: string;
    /** Let OpenRouter fall back to another hoster when the ordered ones fail */
    allow_fallbacks: boolean;
    /** Ask OpenRouter for hosters that support every request parameter */
    require_parameters: boolean;
    temperature: number;
    max_tokens: number;
    timeout_secs: number;
    max_retries: number;
    reasoning_effort: string;
  };
  style: {
    default: string;
    by_app: Record<string, string>;
    custom: CustomStyle[];
  };
  output: {
    mode: string;
    auto_type_max_chars: number;
    restore_clipboard: boolean;
    restore_delay_ms: number;
    paste_backend: string;
    type_backend: string;
    type_delay_ms: number;
    pre_inject_delay_ms: number;
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
    encrypt: boolean;
    key_source: string;
    key_path: string;
    keep_audio: boolean;
    max_entries: number;
    max_size_mb: number;
  };
  stats: { typing_baseline_wpm: number };
  privacy: {
    send_to_llm: boolean;
    cloud_consent: boolean;
    no_record_mode: boolean;
    telemetry: boolean;
  };
  /** The stdio MCP server may be started by a harness; this switch turns it off without
   * touching the harness configuration */
  mcp: { enabled: boolean };
  gui: { tray_animation: boolean; show_managed_settings: boolean };
  log: { level: string; max_size_mb: number };
}

/** Daemon presence: the event `molva://daemon` */
export interface DaemonPresence {
  connected: boolean;
  message?: string;
  hint?: string;
}

export interface TranscribeProgress {
  id: string;
  /** `starting` while the daemon is busy with something else, `engine` while it works on this file */
  phase: "starting" | "engine" | "finalizing";
  /** Empty when the daemon reports no measurable share of the work; never invented */
  percent: number | null;
  started_at_ms: number;
  elapsed_ms: number;
}

/** A file the window may show, as `prepare_media` returns it */
export interface MediaInfo {
  url: string;
  kind: "audio" | "video";
}

/** One entry of the path autocomplete */
export interface PathSuggestion {
  path: string;
  name: string;
  is_dir: boolean;
}
