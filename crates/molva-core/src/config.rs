// SPDX-License-Identifier: MIT
//! Configuration: TOML in the home directory of the user
//!
//! All fields have default values, so an empty file is a valid config, and a partial file is
//! filled in. API keys are not stored in the file: only the name of an environment variable or a
//! keystore

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Config schema version, for future migrations
///
/// 2: an empty `command_mode.system_prompt` means the built-in prompt in the phrase language
pub const CONFIG_VERSION: u32 = 2;

/// The command prompt that version 1 wrote into every settings file by default
///
/// A frozen copy, not a reference to the current built-in prompt: migration has to recognise
/// the value that old files hold, even after the built-in prompt changes
const LEGACY_COMMAND_PROMPT: &str = "Ты правишь фрагмент текста по короткой инструкции \
    пользователя. Тебе дают ИНСТРУКЦИЮ и ТЕКСТ. Примени инструкцию к тексту и верни только \
    получившийся текст: без заголовков, без markdown, без кавычек, без пояснений и без слов \
    вроде «вот результат». Если инструкция — перевод, верни только перевод. Не добавляй \
    ничего, чего нет в тексте, сохраняй язык текста, если инструкция не велит его сменить. \
    Если инструкцию выполнить нельзя, верни текст без изменений.";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error in the settings file {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("could not serialize the settings: {0}")]
    Serialize(String),
    #[error("could not find the settings directory of the user")]
    NoHome,
    #[error("no such setting: {0}")]
    UnknownKey(String),
    #[error("{key} = {value:?}: {message}")]
    BadValue {
        key: String,
        value: String,
        message: String,
    },
    #[error("the settings failed validation:\n{}", .0.iter().map(|i| format!("  - {i}")).collect::<Vec<String>>().join("\n"))]
    Invalid(Vec<ConfigIssue>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// GUI language: `auto` | `ru` | `en`. `auto` takes the system language, and an unknown
    /// system language gives English
    pub ui_language: String,
    pub audio: AudioConfig,
    pub stt: SttConfig,
    pub dictionary: DictionaryConfig,
    pub rules: RulesConfig,
    pub llm: LlmConfig,
    pub style: StyleConfig,
    pub output: OutputConfig,
    pub hotkeys: HotkeysConfig,
    pub command_mode: CommandModeConfig,
    pub journal: JournalConfig,
    pub stats: StatsConfig,
    pub privacy: PrivacyConfig,
    pub autostart: AutostartConfig,
    pub gui: GuiConfig,
    pub log: LogConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            ui_language: "auto".into(),
            audio: AudioConfig::default(),
            stt: SttConfig::default(),
            dictionary: DictionaryConfig::default(),
            rules: RulesConfig::default(),
            llm: LlmConfig::default(),
            style: StyleConfig::default(),
            output: OutputConfig::default(),
            hotkeys: HotkeysConfig::default(),
            command_mode: CommandModeConfig::default(),
            journal: JournalConfig::default(),
            stats: StatsConfig::default(),
            privacy: PrivacyConfig::default(),
            autostart: AutostartConfig::default(),
            gui: GuiConfig::default(),
            log: LogConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Device name from `molva devices`, or `default`
    pub device: String,
    pub gain: f32,
    pub max_duration_secs: u32,
    pub trim_silence: bool,
    pub silence_threshold_db: f32,
    /// A pause shorter than this does not cut a phrase
    pub vad_min_pause_ms: u32,
    pub noise_suppression: bool,
    pub sounds: bool,
    pub sound_volume: f32,
    pub warn_zero_level: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            device: "default".into(),
            gain: 1.0,
            max_duration_secs: 600,
            trim_silence: true,
            silence_threshold_db: -45.0,
            vad_min_pause_ms: 1500,
            noise_suppression: false,
            sounds: true,
            sound_volume: 0.4,
            warn_zero_level: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// `whisper-cpp` | `remote-openai`
    pub engine: String,
    pub model: String,
    /// Empty means the default model directory
    pub model_path: String,
    /// Where to download the model weights instead of Hugging Face: a directory URL, and the file
    /// name is appended to it. Empty means Hugging Face. The SHA-256 check catches a swapped file,
    /// so `http://` is fine too
    pub models_mirror: String,
    /// `auto` or an ISO-639-1 code; a fixed language turns off auto-detection
    pub language: String,
    pub allowed_languages: Vec<String>,
    /// 0 means all logical cores
    pub threads: u32,
    pub unload_after_secs: u32,
    pub no_speech_threshold: f32,
    /// Download the weights at daemon start if they are missing: one command on a clean machine
    /// `false` is the old behaviour, an error with the `molva models pull` command
    pub auto_pull: bool,
    /// Recognize a phrase in chunks right during the recording
    pub chunked: bool,
    /// The pause that cuts a chunk in streaming processing
    pub chunk_pause_ms: u32,
    pub streaming_preview: bool,
    pub remote: RemoteSttConfig,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper-cpp".into(),
            model: "small".into(),
            model_path: String::new(),
            models_mirror: String::new(),
            // Fixed language: auto-detection in whisper.cpp on the CPU is five times slower
            // (23 s against 4 s for a 4 s phrase with the small model). `auto` stays an option
            language: "ru".into(),
            allowed_languages: vec!["ru".into(), "en".into()],
            threads: 0,
            unload_after_secs: 600,
            no_speech_threshold: 0.6,
            auto_pull: true,
            chunked: true,
            // The segmentation pause is shorter than `audio.vad_min_pause_ms`: there the pause
            // decides whether the phrase has ended, and here it decides whether a chunk can
            // already go to the model
            chunk_pause_ms: crate::app::audio::segmenter::DEFAULT_CHUNK_PAUSE_MS,
            streaming_preview: true,
            remote: RemoteSttConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSttConfig {
    pub base_url: String,
    /// `keyring` | `env`
    pub api_key_source: String,
    pub api_key_env: String,
    pub model: String,
}

impl Default for RemoteSttConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key_source: "keyring".into(),
            api_key_env: "GROQ_API_KEY".into(),
            model: "whisper-large-v3-turbo".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DictionaryConfig {
    /// Empty means `dictionary.toml` next to the config
    pub path: String,
    pub fuzzy: bool,
    /// Pass the terms to the whisper prompt
    pub in_prompt: bool,
}

impl Default for DictionaryConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            fuzzy: true,
            in_prompt: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesConfig {
    pub enabled: bool,
    pub spoken_punctuation: bool,
    pub auto_punctuation: bool,
    pub remove_fillers: bool,
    pub remove_repeats: bool,
    pub numbers_as_digits: bool,
    pub paragraph_pause_ms: u32,
    /// Phrases no longer than this are processed by the rules only, with no model
    pub llm_min_words: u32,
}

impl Default for RulesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            spoken_punctuation: true,
            auto_punctuation: true,
            remove_fillers: true,
            remove_repeats: true,
            numbers_as_digits: true,
            paragraph_pause_ms: 2000,
            llm_min_words: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub enabled: bool,
    /// `ollama` | `lmstudio` | `openrouter` | `groq` | `openai` | `custom`
    pub provider: String,
    pub base_url: String,
    pub model: String,
    /// `keyring` is the OS store, `file` is `<settings directory>/secrets/llm-api-key-<provider>`,
    /// `env` is the environment variable only, `none` means no key
    pub api_key_source: String,
    pub api_key_env: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    pub max_retries: u32,
    /// How much the model "thinks" before it answers: `auto` sends `none` for local providers
    /// (Ollama, LM Studio) and does not send the field for cloud ones; an empty string means
    /// never send; any other value (`none`, `low`, `medium`, `high`) goes as it is. Otherwise
    /// reasoning models such as qwen3.5 spend the whole `max_tokens` on reasoning and return
    /// empty text
    pub reasoning_effort: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "ollama".into(),
            base_url: "http://localhost:11434/v1".into(),
            model: "qwen3.5:4b".into(),
            api_key_source: "keyring".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            temperature: 0.2,
            max_tokens: 1024,
            timeout_secs: 20,
            max_retries: 2,
            reasoning_effort: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleConfig {
    pub default: String,
    /// Window class → style identifier
    pub by_app: BTreeMap<String, String>,
    pub custom: Vec<CustomStyle>,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            default: "cleanup".into(),
            by_app: BTreeMap::default(),
            custom: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomStyle {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub uses_llm: bool,
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// `auto` | `paste` | `type` | `clipboard`
    pub mode: String,
    pub auto_type_max_chars: u32,
    pub restore_clipboard: bool,
    pub restore_delay_ms: u32,
    pub paste_backend: String,
    pub type_backend: String,
    pub type_delay_ms: u32,
    /// Pause before the paste, in milliseconds
    ///
    /// Between the key release and the paste the window must have time to return the focus to the
    /// input field. Locally 50 ms is enough; a remote desktop (RDP, VNC, xrdp) and heavy Electron
    /// windows need clearly more — up to 1500 ms
    pub pre_inject_delay_ms: u32,
    /// Paste into terminals with Ctrl+Shift+V, by looking at the class of the active window
    pub terminal_shortcut: bool,
    pub notify_on_fallback: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            mode: "auto".into(),
            auto_type_max_chars: 200,
            restore_clipboard: true,
            restore_delay_ms: 400,
            paste_backend: "auto".into(),
            type_backend: "auto".into(),
            type_delay_ms: 4,
            pre_inject_delay_ms: 50,
            // Terminals are a usual place for dictation, and Ctrl+V pastes nothing in them:
            // by default we look at the window class and switch to Ctrl+Shift+V
            terminal_shortcut: true,
            notify_on_fallback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeysConfig {
    /// `auto` | `external` | `evdev` | `gui`
    pub backend: String,
    pub push_to_talk: String,
    pub toggle: String,
    pub command: String,
    pub cancel: String,
    pub style_next: String,
    pub tap_toggles: bool,
    pub short_press_ms: u32,
    /// A key hold shorter than this does not create a phrase
    pub min_hold_ms: u32,
    pub double_tap_ms: u32,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
            // The default key combinations are unergonomic on purpose: F9 and Ctrl+Alt are free in
            // almost any system and application, so on a foreign test machine they work with no
            // setup. While the application is in testing, this matters more than comfort; the
            // keys change to fit the hand in this same section
            push_to_talk: "F9".into(),
            toggle: "Ctrl+Alt+Z".into(),
            command: "Ctrl+Alt+X".into(),
            cancel: "Ctrl+Alt+C".into(),
            style_next: "Ctrl+Alt+S".into(),
            tap_toggles: true,
            short_press_ms: 250,
            min_hold_ms: 200,
            double_tap_ms: 350,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandModeConfig {
    pub enabled: bool,
    /// Empty: the built-in prompt in the language of the instruction. Set: this prompt for every
    /// language
    pub system_prompt: String,
}

impl Default for CommandModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            system_prompt: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JournalConfig {
    /// Empty means `$XDG_DATA_HOME/molva/journal.jsonl`
    pub path: String,
    pub enabled: bool,
    /// `false` is privacy mode: a line with no text of the phrase
    pub include_text: bool,
    /// `true` means the texts of the phrases in the journal are encrypted (XChaCha20-Poly1305), and
    /// the key comes from `key_source`. The other fields of the line stay open: statistics and
    /// filters work without the key
    pub encrypt: bool,
    /// Where the key lives: `file` is the `key_path` file, `keyring` is the `journal-key` entry
    /// in the OS store. In both cases the key is created by itself at the first open
    pub key_source: String,
    /// Encryption key file; empty means `journal.key` next to the journal. It is created by
    /// itself, with permissions for the owner only
    pub key_path: String,
    pub keep_audio: bool,
    pub max_entries: u32,
    pub max_size_mb: u32,
}

impl Default for JournalConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            enabled: true,
            include_text: true,
            encrypt: false,
            key_source: "file".into(),
            key_path: String::new(),
            keep_audio: false,
            max_entries: 10_000,
            max_size_mb: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsConfig {
    /// Baseline for the "saved time" against typing on a keyboard
    pub typing_baseline_wpm: u32,
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            typing_baseline_wpm: 40,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub send_to_llm: bool,
    /// Turns off the text journal and the whole history
    pub no_record_mode: bool,
    /// There is no telemetry; the key exists to say this explicitly
    pub telemetry: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            send_to_llm: true,
            no_record_mode: false,
            telemetry: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutostartConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    /// Animate the tray icon during the recording; `false` by default
    ///
    /// It is off on purpose: the icon changes only when the state changes (idle, recording,
    /// processing), so it updates only a few times per phrase, and not on every signal level
    /// event. Whoever needs a blinking indicator more than a calm tray sets `true`
    pub tray_animation: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// `error` | `warn` | `info` | `debug` | `trace`
    pub level: String,
    pub max_size_mb: u32,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            max_size_mb: 10,
        }
    }
}

fn default_true() -> bool {
    true
}

/// A value from a closed list; case does not matter
fn one_of(issues: &mut Vec<ConfigIssue>, key: &str, value: &str, allowed: &[&str]) {
    if !allowed.iter().any(|a| a.eq_ignore_ascii_case(value)) {
        issues.push(ConfigIssue::allowed(key, value, allowed));
    }
}

/// A number in a closed range, the bounds are included
fn in_range(issues: &mut Vec<ConfigIssue>, key: &str, value: f64, lo: f64, hi: f64) {
    if value < lo || value > hi {
        issues.push(ConfigIssue::range(key, &format!("{value}"), lo, hi));
    }
}

impl Config {
    /// Settings directory of the user: `~/.config/molva` on Linux and the equivalents on other OSes
    pub fn default_dir() -> Result<PathBuf, ConfigError> {
        directories::ProjectDirs::from("", "", "molva")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .ok_or(ConfigError::NoHome)
    }

    /// Path to the settings file; `MOLVA_CONFIG` overrides it
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        if let Some(path) = std::env::var_os("MOLVA_CONFIG") {
            return Ok(PathBuf::from(path));
        }
        Ok(Self::default_dir()?.join("config.toml"))
    }

    pub fn from_toml_str(path: &Path, text: &str) -> Result<Self, ConfigError> {
        toml::from_str(text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })
    }

    pub fn to_toml_string(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::Serialize(e.to_string()))
    }

    /// Read the file; a missing file means the default settings
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml_str(path, &text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Read the file, and if it does not exist, create it with the default values
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            let config = Self::default();
            config.save(path)?;
            return Ok(config);
        }
        Self::load(path)
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: path.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(path, self.to_toml_string()?).map_err(|source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Data directory of the user: the journal, the models, the saved audio
    ///
    /// `MOLVA_DATA_DIR` overrides it — this makes it easy to run several profiles and tests by hand
    pub fn data_dir() -> Result<PathBuf, ConfigError> {
        if let Some(directory) = std::env::var_os("MOLVA_DATA_DIR") {
            return Ok(PathBuf::from(directory));
        }
        directories::ProjectDirs::from("", "", "molva")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .ok_or(ConfigError::NoHome)
    }

    /// Path to the journal: from the settings, or `<data_dir>/journal.jsonl`
    pub fn journal_path(&self) -> Result<PathBuf, ConfigError> {
        if !self.journal.path.trim().is_empty() {
            return Ok(PathBuf::from(&self.journal.path));
        }
        Ok(Self::data_dir()?.join("journal.jsonl"))
    }

    /// Journal encryption key file: from the settings, or `journal.key` next to the journal
    pub fn journal_key_path(&self) -> Result<PathBuf, ConfigError> {
        if !self.journal.key_path.trim().is_empty() {
            return Ok(PathBuf::from(&self.journal.key_path));
        }
        let journal = self.journal_path()?;
        Ok(journal.parent().map_or_else(
            || PathBuf::from("journal.key"),
            |dir| dir.join("journal.key"),
        ))
    }

    /// Directory of the file secret store: `<settings directory>/secrets`, one file per entry
    pub fn secrets_directory() -> Result<PathBuf, ConfigError> {
        Ok(Self::default_dir()?.join("secrets"))
    }

    /// Path to the dictionary: from the settings, or `dictionary.toml` next to the settings file
    pub fn dictionary_path(&self) -> Result<PathBuf, ConfigError> {
        self.dictionary_path_near(&Self::default_path()?)
    }

    /// The same, but next to a given settings file: `--config` must move the dictionary too
    pub fn dictionary_path_near(&self, config_path: &Path) -> Result<PathBuf, ConfigError> {
        if !self.dictionary.path.trim().is_empty() {
            return Ok(PathBuf::from(&self.dictionary.path));
        }
        let directory = match config_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => Self::default_dir()?,
        };
        Ok(directory.join("dictionary.toml"))
    }

    /// Read the file, and move a damaged one to `<path>.broken` and start with the defaults
    ///
    /// Returns the settings and a warning if the file had to be replaced: it is wrong to lose
    /// someone's settings silently, but it is also wrong to crash at startup because of one extra
    /// bracket
    pub fn load_lenient(path: &Path) -> Result<(Self, Option<String>), ConfigError> {
        match Self::load(path) {
            Ok(mut config) => {
                let migrated = config.migrate();
                if migrated {
                    config.save(path)?;
                }
                Ok((config, None))
            }
            Err(ConfigError::Parse { message, .. }) => {
                let mut broken = path.as_os_str().to_os_string();
                broken.push(".broken");
                let broken = PathBuf::from(broken);
                std::fs::rename(path, &broken).map_err(|source| ConfigError::Write {
                    path: broken.clone(),
                    source,
                })?;
                let config = Self::default();
                config.save(path)?;
                let warning = format!(
                    "the settings file is damaged ({message}); it is saved as {} and replaced \
                     with the default values",
                    broken.display()
                );
                tracing::warn!("{warning}");
                Ok((config, Some(warning)))
            }
            Err(other) => Err(other),
        }
    }

    /// Bring the settings to the current schema version. `true` means something changed
    pub fn migrate(&mut self) -> bool {
        if self.version == CONFIG_VERSION {
            return false;
        }
        if self.version > CONFIG_VERSION {
            tracing::warn!(
                version = self.version,
                supported = CONFIG_VERSION,
                "the settings file is from a newer version of MolvAI, reading it as it is"
            );
            return false;
        }
        // Version 0 is a file created before the field existed: some fields were missing, serde
        // has already filled in the default values, and only the schema mark remains
        if self.version < 2 && self.command_mode.system_prompt == LEGACY_COMMAND_PROMPT {
            // The file holds the old default, not a choice of the user: let the built-in
            // prompt follow the language of the instruction
            self.command_mode.system_prompt.clear();
        }
        self.version = CONFIG_VERSION;
        true
    }

    /// Validate the whole settings. All errors are collected at once, not one per run
    ///
    /// The order of the issues follows the order of the sections in the settings file: this makes
    /// them easier to match with what the user sees in the editor
    pub fn validate(&self) -> Result<(), Vec<ConfigIssue>> {
        let mut issues = Vec::new();
        self.validate_general(&mut issues);
        self.validate_audio(&mut issues);
        self.validate_stt(&mut issues);
        self.validate_output(&mut issues);
        self.validate_llm(&mut issues);
        self.validate_styles(&mut issues);
        self.validate_limits(&mut issues);

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }

    /// General application settings and the journal
    fn validate_general(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(
            issues,
            "ui_language",
            &self.ui_language,
            &["auto", "ru", "en"],
        );
        one_of(
            issues,
            "journal.key_source",
            &self.journal.key_source,
            &["file", "keyring"],
        );
    }

    /// Audio capture: gain, duration, cue volume
    fn validate_audio(&self, issues: &mut Vec<ConfigIssue>) {
        in_range(issues, "audio.gain", f64::from(self.audio.gain), 0.1, 10.0);
        in_range(
            issues,
            "audio.max_duration_secs",
            f64::from(self.audio.max_duration_secs),
            1.0,
            7200.0,
        );
        in_range(
            issues,
            "audio.sound_volume",
            f64::from(self.audio.sound_volume),
            0.0,
            1.0,
        );
    }

    /// Recognition: engine, language, thresholds, key of the remote API
    fn validate_stt(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(
            issues,
            "stt.engine",
            &self.stt.engine,
            &["whisper-cpp", "remote-openai"],
        );
        if self.stt.language != "auto" && self.stt.language.chars().count() != 2 {
            issues.push(ConfigIssue::new(
                "stt.language",
                &self.stt.language,
                "expected `auto` or a two-letter language code, for example `ru`",
            ));
        }
        in_range(
            issues,
            "stt.no_speech_threshold",
            f64::from(self.stt.no_speech_threshold),
            0.0,
            1.0,
        );
        in_range(
            issues,
            "stt.chunk_pause_ms",
            f64::from(self.stt.chunk_pause_ms),
            100.0,
            5_000.0,
        );
        one_of(
            issues,
            "stt.remote.api_key_source",
            &self.stt.remote.api_key_source,
            &["keyring", "env", "none"],
        );
        let mirror = self.stt.models_mirror.trim();
        if !mirror.is_empty() && !mirror.starts_with("https://") && !mirror.starts_with("http://") {
            issues.push(ConfigIssue::new(
                "stt.models_mirror",
                mirror,
                "expected empty (Hugging Face) or a URL that starts with https:// or http://",
            ));
        }
    }

    /// Delivery of text to the active window
    fn validate_output(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(
            issues,
            "output.mode",
            &self.output.mode,
            &["auto", "paste", "type", "clipboard"],
        );
        in_range(
            issues,
            "output.pre_inject_delay_ms",
            f64::from(self.output.pre_inject_delay_ms),
            0.0,
            5000.0,
        );
        if self.output.auto_type_max_chars == 0 {
            issues.push(ConfigIssue::new(
                "output.auto_type_max_chars",
                "0",
                "expected a positive number of characters",
            ));
        }
    }

    /// Post-processing by a model: provider, key, limits, timeouts
    fn validate_llm(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(
            issues,
            "llm.provider",
            &self.llm.provider,
            &[
                "ollama",
                "lmstudio",
                "lm-studio",
                "openrouter",
                "groq",
                "openai",
                "custom",
            ],
        );
        one_of(
            issues,
            "llm.api_key_source",
            &self.llm.api_key_source,
            &["keyring", "file", "env", "none"],
        );
        in_range(
            issues,
            "llm.temperature",
            f64::from(self.llm.temperature),
            0.0,
            2.0,
        );
        in_range(
            issues,
            "llm.max_tokens",
            f64::from(self.llm.max_tokens),
            1.0,
            32_768.0,
        );
        in_range(
            issues,
            "llm.timeout_secs",
            self.llm.timeout_secs as f64,
            1.0,
            600.0,
        );
        in_range(
            issues,
            "llm.max_retries",
            f64::from(self.llm.max_retries),
            0.0,
            10.0,
        );
        if self.llm.enabled && self.llm.base_url.trim().is_empty() {
            issues.push(ConfigIssue::new(
                "llm.base_url",
                "",
                "post-processing is on, but the model URL is not set",
            ));
        }
    }

    /// Post-processing styles: both the default and the bindings to applications
    fn validate_styles(&self, issues: &mut Vec<ConfigIssue>) {
        let styles = crate::app::styles::Styles::from_config(&self.style);
        if styles.get(&self.style.default).is_none() {
            let known: Vec<&str> = styles.all().iter().map(|s| s.id.as_str()).collect();
            issues.push(ConfigIssue::allowed(
                "style.default",
                &self.style.default,
                &known,
            ));
        }
        for (app, style) in &self.style.by_app {
            if styles.get(style).is_none() {
                issues.push(ConfigIssue::new(
                    &format!("style.by_app.{app}"),
                    style,
                    "no such style: see `molva styles list`",
                ));
            }
        }
    }

    /// The remaining numeric thresholds, the log level and the hotkeys
    fn validate_limits(&self, issues: &mut Vec<ConfigIssue>) {
        in_range(
            issues,
            "rules.llm_min_words",
            f64::from(self.rules.llm_min_words),
            0.0,
            1000.0,
        );
        in_range(
            issues,
            "stats.typing_baseline_wpm",
            f64::from(self.stats.typing_baseline_wpm),
            1.0,
            400.0,
        );
        one_of(
            issues,
            "log.level",
            &self.log.level,
            &["error", "warn", "info", "debug", "trace"],
        );
        if self.hotkeys.push_to_talk.trim().is_empty() {
            issues.push(ConfigIssue::new(
                "hotkeys.push_to_talk",
                "",
                "the key hold key is not set: there will be nothing to dictate with",
            ));
        }
    }

    /// All setting keys as dotted paths — for `molva config get` and for hints
    pub fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if let Ok(value) = toml::Value::try_from(self) {
            collect_keys(&value, "", &mut keys);
        }
        keys.sort();
        keys
    }

    /// Value at a path such as `stt.model`, as it would look in TOML (strings without quotes)
    pub fn get_by_path(&self, path: &str) -> Result<String, ConfigError> {
        let root =
            toml::Value::try_from(self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        let value = value_at(&root, path).ok_or_else(|| ConfigError::UnknownKey(path.into()))?;
        Ok(render(value))
    }

    /// Set the value at a path; the type comes from the current value, and the result is validated
    pub fn set_by_path(&mut self, path: &str, raw: &str) -> Result<(), ConfigError> {
        let mut root =
            toml::Value::try_from(&*self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        let existing = value_at(&root, path)
            .ok_or_else(|| ConfigError::UnknownKey(path.into()))?
            .clone();
        let parsed = parse_like(&existing, raw, path)?;
        set_value_at(&mut root, path, parsed)?;
        let updated: Config =
            root.try_into()
                .map_err(|e: toml::de::Error| ConfigError::BadValue {
                    key: path.to_string(),
                    value: raw.to_string(),
                    message: e.to_string(),
                })?;
        updated.validate().map_err(ConfigError::Invalid)?;
        *self = updated;
        Ok(())
    }

    /// Write the settings to a file — the same format as the working config
    pub fn export(&self, path: &Path) -> Result<(), ConfigError> {
        self.save(path)
    }

    /// Load the settings from a profile file; a damaged or invalid file is not applied
    pub fn import(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config = Self::from_toml_str(path, &text)?;
        config.migrate();
        config.validate().map_err(ConfigError::Invalid)?;
        Ok(config)
    }
}

/// One complaint about the settings: where, what is set and what was expected
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub key: String,
    pub value: String,
    pub message: String,
}

impl ConfigIssue {
    pub fn new(key: &str, value: &str, message: &str) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
            message: message.to_string(),
        }
    }

    fn allowed(key: &str, value: &str, allowed: &[&str]) -> Self {
        Self::new(
            key,
            value,
            &format!("allowed values: {}", allowed.join(", ")),
        )
    }

    fn range(key: &str, value: &str, lo: f64, hi: f64) -> Self {
        Self::new(key, value, &format!("expected a number from {lo} to {hi}"))
    }
}

impl std::fmt::Display for ConfigIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} = {:?}: {}", self.key, self.value, self.message)
    }
}

fn collect_keys(value: &toml::Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_keys(child, &path, out);
            }
        }
        _ => {
            if !prefix.is_empty() {
                out.push(prefix.to_string());
            }
        }
    }
}

fn value_at<'a>(value: &'a toml::Value, path: &str) -> Option<&'a toml::Value> {
    let mut current = value;
    for part in path.split('.') {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

fn set_value_at(root: &mut toml::Value, path: &str, new: toml::Value) -> Result<(), ConfigError> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = root;
    for part in &parts[..parts.len() - 1] {
        current = current
            .as_table_mut()
            .and_then(|table| table.get_mut(*part))
            .ok_or_else(|| ConfigError::UnknownKey(path.into()))?;
    }
    let table = current
        .as_table_mut()
        .ok_or_else(|| ConfigError::UnknownKey(path.into()))?;
    let last = parts[parts.len() - 1];
    table.insert(last.to_string(), new);
    Ok(())
}

/// How a value looks in the `config get` output: a string without quotes, everything else as in
/// TOML
fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Array(items) => items.iter().map(render).collect::<Vec<String>>().join(", "),
        other => other.to_string(),
    }
}

/// Parse a command-line string into the type of the current value
fn parse_like(existing: &toml::Value, raw: &str, key: &str) -> Result<toml::Value, ConfigError> {
    let bad = |message: &str| ConfigError::BadValue {
        key: key.to_string(),
        value: raw.to_string(),
        message: message.to_string(),
    };
    match existing {
        toml::Value::String(_) => Ok(toml::Value::String(raw.to_string())),
        toml::Value::Integer(_) => raw
            .trim()
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|_| bad("expected an integer")),
        toml::Value::Float(_) => raw
            .trim()
            .parse::<f64>()
            .map(toml::Value::Float)
            .map_err(|_| bad("expected a number, for example 0.4")),
        toml::Value::Boolean(_) => match raw.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "да" | "on" => Ok(toml::Value::Boolean(true)),
            "false" | "0" | "no" | "нет" | "off" => Ok(toml::Value::Boolean(false)),
            _ => Err(bad("expected true or false")),
        },
        toml::Value::Array(_) => Ok(toml::Value::Array(
            raw.split(',')
                .map(|item| toml::Value::String(item.trim().to_string()))
                .filter(|item| item.as_str().is_some_and(|s| !s.is_empty()))
                .collect(),
        )),
        toml::Value::Table(_) => Err(bad("this is a settings section, not a value")),
        toml::Value::Datetime(_) => Err(bad(
            "a value of this type cannot be changed from the command line",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let config = Config::default();
        let text = config.to_toml_string().unwrap();
        let back = Config::from_toml_str(Path::new("x.toml"), &text).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn partial_file_is_filled_with_defaults() {
        let text = "[stt]\nmodel = \"large-v3-turbo\"\n[llm]\nenabled = true\n";
        let config = Config::from_toml_str(Path::new("x.toml"), text).unwrap();
        assert_eq!(config.stt.model, "large-v3-turbo");
        assert_eq!(config.stt.language, "ru");
        assert!(config.llm.enabled);
        assert_eq!(config.output.auto_type_max_chars, 200);
    }

    #[test]
    fn empty_file_is_default_config() {
        let config = Config::from_toml_str(Path::new("x.toml"), "").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn invalid_value_reports_path_and_reason() {
        let err = Config::from_toml_str(Path::new("/tmp/c.toml"), "[audio]\ngain = \"loud\"\n")
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("/tmp/c.toml"), "{message}");
        assert!(message.contains("gain"), "{message}");
    }

    #[test]
    fn load_or_create_writes_defaults_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("config.toml");
        let created = Config::load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(created, Config::default());
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, created);
    }

    #[test]
    fn missing_file_loads_as_defaults_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert!(!path.exists());
    }

    #[test]
    fn no_api_key_field_exists_in_config() {
        let text = Config::default().to_toml_string().unwrap();
        assert!(
            !text.contains("api_key ="),
            "the key must not be stored in the file"
        );
        assert!(text.contains("api_key_env"));
    }

    #[test]
    fn defaults_pass_validation() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn streaming_is_on_out_of_the_box() {
        // Promise of the product: the draft is visible during speech, and after the release only a
        // tail remains. It can be turned off only explicitly in the settings file
        let stt = Config::default().stt;
        assert!(stt.chunked, "streaming processing is off by default");
        assert!(stt.streaming_preview, "the draft is off by default");
        assert_eq!(stt.chunk_pause_ms, 700);
    }

    #[test]
    fn an_impossible_chunk_pause_is_refused() {
        let mut config = Config::default();
        config.stt.chunk_pause_ms = 10;
        let issues = config.validate().unwrap_err();
        assert!(
            issues
                .iter()
                .any(|i| i.to_string().contains("chunk_pause_ms")),
            "{issues:?}"
        );
    }

    #[test]
    fn a_models_mirror_must_be_an_http_address() {
        let mut config = Config::default();
        for good in [
            "",
            "https://hf-mirror.com/whisper",
            "http://192.168.1.5:8000/models",
        ] {
            config.stt.models_mirror = good.into();
            assert!(config.validate().is_ok(), "{good}");
        }
        config.stt.models_mirror = "hf-mirror.com/whisper".into();
        let issues = config.validate().unwrap_err();
        assert!(
            issues
                .iter()
                .any(|i| i.to_string().contains("stt.models_mirror")),
            "{issues:?}"
        );
    }

    #[test]
    fn a_wrong_value_names_the_key_the_value_and_what_was_expected() {
        let mut config = Config::default();
        config.output.mode = "pasta".into();
        let issues = config.validate().unwrap_err();
        assert_eq!(issues.len(), 1, "{issues:?}");
        let text = issues[0].to_string();
        assert!(text.contains("output.mode"), "{text}");
        assert!(text.contains("pasta"), "{text}");
        assert!(text.contains("auto, paste, type, clipboard"), "{text}");
    }

    #[test]
    fn interface_language_follows_the_system_out_of_the_box() {
        let config = Config::default();
        assert_eq!(config.ui_language, "auto");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn every_broken_key_is_reported_at_once() {
        let mut config = Config {
            ui_language: "kz".into(),
            ..Config::default()
        };
        config.log.level = "loud".into();
        config.stats.typing_baseline_wpm = 0;
        config.llm.temperature = 9.0;
        let issues = config.validate().unwrap_err();
        let keys: Vec<&str> = issues.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"ui_language"), "{keys:?}");
        assert!(keys.contains(&"log.level"), "{keys:?}");
        assert!(keys.contains(&"stats.typing_baseline_wpm"), "{keys:?}");
        assert!(keys.contains(&"llm.temperature"), "{keys:?}");
    }

    #[test]
    fn an_unknown_style_in_the_settings_is_reported() {
        let mut config = Config::default();
        config.style.default = "poem".into();
        config
            .style
            .by_app
            .insert("kitty".into(), "no such either".into());
        let issues = config.validate().unwrap_err();
        let keys: Vec<&str> = issues.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"style.default"), "{keys:?}");
        assert!(keys.contains(&"style.by_app.kitty"), "{keys:?}");
    }

    #[test]
    fn a_custom_style_counts_as_a_known_one() {
        let mut config = Config::default();
        config.style.custom.push(CustomStyle {
            id: "poem".into(),
            name: "Poem".into(),
            uses_llm: true,
            system_prompt: "Пиши стихами.".into(),
        });
        config.style.default = "poem".into();
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn values_are_read_by_dotted_path() {
        let config = Config::default();
        assert_eq!(config.get_by_path("stt.model").unwrap(), "small");
        assert_eq!(
            config.get_by_path("output.auto_type_max_chars").unwrap(),
            "200"
        );
        assert_eq!(config.get_by_path("llm.enabled").unwrap(), "false");
        assert_eq!(
            config.get_by_path("stt.allowed_languages").unwrap(),
            "ru, en"
        );
        assert!(matches!(
            config.get_by_path("stt.no_such_key").unwrap_err(),
            ConfigError::UnknownKey(_)
        ));
    }

    #[test]
    fn values_are_written_by_dotted_path_in_the_right_type() {
        let mut config = Config::default();
        config.set_by_path("output.mode", "paste").unwrap();
        assert_eq!(config.output.mode, "paste");
        config
            .set_by_path("output.auto_type_max_chars", "120")
            .unwrap();
        assert_eq!(config.output.auto_type_max_chars, 120);
        config.set_by_path("llm.enabled", "true").unwrap();
        assert!(config.llm.enabled);
        config.set_by_path("audio.gain", "1.5").unwrap();
        assert!((config.audio.gain - 1.5).abs() < 1e-6);
        config
            .set_by_path("stt.allowed_languages", "ru, de")
            .unwrap();
        assert_eq!(config.stt.allowed_languages, vec!["ru", "de"]);
    }

    #[test]
    fn a_wrong_type_is_refused_and_nothing_changes() {
        let mut config = Config::default();
        let before = config.clone();
        let err = config
            .set_by_path("output.auto_type_max_chars", "many")
            .unwrap_err();
        assert!(err.to_string().contains("integer"), "{err}");
        assert_eq!(config, before);
    }

    #[test]
    fn a_value_that_fails_validation_is_refused_and_nothing_changes() {
        let mut config = Config::default();
        let before = config.clone();
        let err = config.set_by_path("log.level", "loud").unwrap_err();
        assert!(err.to_string().contains("log.level"), "{err}");
        assert_eq!(config, before);
    }

    #[test]
    fn a_section_cannot_be_assigned_a_value() {
        let mut config = Config::default();
        let err = config.set_by_path("llm", "anything").unwrap_err();
        assert!(err.to_string().contains("section"), "{err}");
    }

    #[test]
    fn keys_list_every_leaf_and_no_sections() {
        let keys = Config::default().keys();
        assert!(keys.contains(&"stt.model".to_string()));
        assert!(keys.contains(&"llm.timeout_secs".to_string()));
        assert!(keys.contains(&"privacy.no_record_mode".to_string()));
        assert!(!keys.contains(&"llm".to_string()));
        // The list is sorted, so the `config get` output is stable
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn a_broken_file_is_moved_aside_and_replaced_by_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[llm]\nenabled = = true\n").unwrap();

        let (config, warning) = Config::load_lenient(&path).unwrap();
        assert_eq!(config, Config::default());
        let warning = warning.expect("the user must learn about the replacement");
        assert!(warning.contains("damaged"), "{warning}");

        let broken = directory.path().join("config.toml.broken");
        assert!(broken.exists(), "the damaged file must be kept");
        assert!(std::fs::read_to_string(&broken)
            .unwrap()
            .contains("= = true"));
        // A working file now stands in place of the settings
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn a_healthy_file_is_left_alone_by_load_lenient() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "version = 1\n[stt]\nmodel = \"large-v3-turbo\"\n").unwrap();
        let (config, warning) = Config::load_lenient(&path).unwrap();
        assert_eq!(config.stt.model, "large-v3-turbo");
        assert_eq!(warning, None);
        assert!(!directory.path().join("config.toml.broken").exists());
    }

    #[test]
    fn migration_stamps_the_schema_version_on_an_old_file() {
        let mut config = Config::from_toml_str(Path::new("x.toml"), "version = 0\n").unwrap();
        assert!(config.migrate());
        assert_eq!(config.version, CONFIG_VERSION);
        // A repeated migration does nothing
        assert!(!config.migrate());
    }

    /// A file saved by version 1 with the command prompt it had by default
    fn version_1_file(prompt: &str) -> Config {
        let mut config = Config {
            version: 1,
            ..Config::default()
        };
        config.command_mode.system_prompt = prompt.into();
        config
    }

    #[test]
    fn the_old_default_command_prompt_gives_way_to_the_built_in_one_in_the_phrase_language() {
        let mut config = version_1_file(LEGACY_COMMAND_PROMPT);
        assert!(config.migrate());
        assert_eq!(config.command_mode.system_prompt, "");
        assert_eq!(config.version, CONFIG_VERSION);
        // A Russian instruction gets exactly the prompt the file had: nothing changes for it
        assert_eq!(
            crate::app::command_mode::system_prompt(&config.command_mode.system_prompt, "ru"),
            LEGACY_COMMAND_PROMPT
        );
    }

    #[test]
    fn a_command_prompt_of_the_user_survives_the_migration() {
        let mut config = version_1_file("Answer in verse.");
        assert!(config.migrate());
        assert_eq!(config.command_mode.system_prompt, "Answer in verse.");
    }

    #[test]
    fn a_new_config_uses_the_built_in_command_prompt() {
        assert_eq!(Config::default().command_mode.system_prompt, "");
    }

    #[test]
    fn a_file_from_the_future_is_read_as_is_without_downgrading() {
        let mut config = Config {
            version: CONFIG_VERSION + 5,
            ..Config::default()
        };
        assert!(!config.migrate());
        assert_eq!(config.version, CONFIG_VERSION + 5);
    }

    #[test]
    fn a_profile_round_trips_through_export_and_import() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.toml");
        let mut config = Config::default();
        config.stt.model = "large-v3-turbo".into();
        config.output.mode = "paste".into();
        config.export(&path).unwrap();

        let imported = Config::import(&path).unwrap();
        assert_eq!(imported, config);
    }

    #[test]
    fn an_invalid_profile_is_not_imported() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.toml");
        std::fs::write(&path, "[output]\nmode = \"telepathy\"\n").unwrap();
        let err = Config::import(&path).unwrap_err();
        assert!(err.to_string().contains("output.mode"), "{err}");
    }

    #[test]
    fn the_dictionary_lives_next_to_the_settings_file_it_belongs_to() {
        let config = Config::default();
        assert_eq!(
            config
                .dictionary_path_near(Path::new("/opt/profiles/work/config.toml"))
                .unwrap(),
            Path::new("/opt/profiles/work/dictionary.toml")
        );
    }

    #[test]
    fn an_explicit_dictionary_path_wins_over_the_settings_directory() {
        let mut config = Config::default();
        config.dictionary.path = "/srv/terms.toml".into();
        assert_eq!(
            config
                .dictionary_path_near(Path::new("/opt/profiles/work/config.toml"))
                .unwrap(),
            Path::new("/srv/terms.toml")
        );
    }

    #[test]
    fn the_journal_path_comes_from_the_settings_when_it_is_set() {
        let mut config = Config::default();
        config.journal.path = "/tmp/molva-test/journal.jsonl".into();
        assert_eq!(
            config.journal_path().unwrap(),
            Path::new("/tmp/molva-test/journal.jsonl")
        );
    }
}
