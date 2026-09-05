// SPDX-License-Identifier: MIT
//! Конфигурация: TOML в домашнем каталоге пользователя.
//!
//! Все поля имеют значения по умолчанию, поэтому пустой файл — валидный конфиг, а частичный
//! файл дополняется. Ключи API в файле не хранятся: только имя переменной окружения или keystore.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Версия схемы конфига, для будущих миграций.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("не удалось прочитать {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("не удалось записать {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ошибка в файле настроек {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("не удалось сериализовать настройки: {0}")]
    Serialize(String),
    #[error("не удалось определить каталог настроек пользователя")]
    NoHome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// Язык интерфейса CLI и GUI: `ru` | `en`.
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
    pub log: LogConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            ui_language: "ru".into(),
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
            log: LogConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Имя устройства из `molva devices` или `default`.
    pub device: String,
    pub gain: f32,
    pub max_duration_secs: u32,
    pub trim_silence: bool,
    pub silence_threshold_db: f32,
    /// Пауза короче этого не режет реплику.
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
    /// `whisper-cpp` | `remote-openai`.
    pub engine: String,
    pub model: String,
    /// Пусто — каталог моделей по умолчанию.
    pub model_path: String,
    /// `auto` или код ISO-639-1; фиксированный язык отключает автоопределение.
    pub language: String,
    pub allowed_languages: Vec<String>,
    /// 0 — все логические ядра.
    pub threads: u32,
    pub unload_after_secs: u32,
    pub no_speech_threshold: f32,
    pub streaming_preview: bool,
    pub remote: RemoteSttConfig,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            engine: "whisper-cpp".into(),
            model: "small".into(),
            model_path: String::new(),
            language: "auto".into(),
            allowed_languages: vec!["ru".into(), "en".into()],
            threads: 0,
            unload_after_secs: 600,
            no_speech_threshold: 0.6,
            streaming_preview: false,
            remote: RemoteSttConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSttConfig {
    pub base_url: String,
    /// `keyring` | `env`.
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
    /// Пусто — `dictionary.toml` рядом с конфигом.
    pub path: String,
    pub fuzzy: bool,
    /// Передавать термины в подсказку whisper.
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
    /// Реплики не длиннее этого обрабатываются только правилами, без модели.
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
    /// `ollama` | `lmstudio` | `openrouter` | `groq` | `openai` | `custom`.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    /// `keyring` | `env`.
    pub api_key_source: String,
    pub api_key_env: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub timeout_secs: u64,
    pub max_retries: u32,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StyleConfig {
    pub default: String,
    /// Класс окна → идентификатор стиля.
    pub by_app: std::collections::BTreeMap<String, String>,
    pub custom: Vec<CustomStyle>,
}

impl Default for StyleConfig {
    fn default() -> Self {
        Self {
            default: "cleanup".into(),
            by_app: Default::default(),
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
    /// `auto` | `paste` | `type` | `clipboard`.
    pub mode: String,
    pub auto_type_max_chars: u32,
    pub restore_clipboard: bool,
    pub restore_delay_ms: u32,
    pub paste_backend: String,
    pub type_backend: String,
    pub type_delay_ms: u32,
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
            terminal_shortcut: false,
            notify_on_fallback: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotkeysConfig {
    /// `auto` | `external` | `evdev` | `gui`.
    pub backend: String,
    pub push_to_talk: String,
    pub toggle: String,
    pub command: String,
    pub cancel: String,
    pub style_next: String,
    pub tap_toggles: bool,
    pub short_press_ms: u32,
    /// Удержание короче этого не создаёт реплику.
    pub min_hold_ms: u32,
    pub double_tap_ms: u32,
}

impl Default for HotkeysConfig {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
            push_to_talk: "RightCtrl".into(),
            toggle: "Ctrl+Shift+Space".into(),
            command: "Ctrl+Shift+Alt+Space".into(),
            cancel: "Escape".into(),
            style_next: "Ctrl+Shift+Alt+S".into(),
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
    pub system_prompt: String,
}

impl Default for CommandModeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            system_prompt: "Ты редактируешь текст. Применяй голосовую инструкцию к ВЫДЕЛЕНИЮ. \
                            Выводи только результат."
                .into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JournalConfig {
    /// Пусто — `$XDG_DATA_HOME/molva/journal.jsonl`.
    pub path: String,
    pub enabled: bool,
    /// `false` — режим приватности: строка без текста реплики.
    pub include_text: bool,
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
            keep_audio: false,
            max_entries: 10_000,
            max_size_mb: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsConfig {
    /// База для «сэкономленного времени» против набора с клавиатуры.
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
    /// Отключает журнал текста и историю целиком.
    pub no_record_mode: bool,
    /// Телеметрии нет; ключ существует, чтобы это заявить явно.
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    /// `error` | `warn` | `info` | `debug` | `trace`.
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

impl Config {
    /// Каталог настроек пользователя: `~/.config/molva` на Linux и аналоги на других ОС.
    pub fn default_dir() -> Result<PathBuf, ConfigError> {
        directories::ProjectDirs::from("", "", "molva")
            .map(|dirs| dirs.config_dir().to_path_buf())
            .ok_or(ConfigError::NoHome)
    }

    /// Путь к файлу настроек; `MOLVA_CONFIG` переопределяет.
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

    /// Прочитать файл; отсутствующий файл — это настройки по умолчанию.
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

    /// Прочитать файл, а если его нет — создать со значениями по умолчанию.
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
        assert_eq!(config.stt.language, "auto");
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let created = Config::load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(created, Config::default());
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, created);
    }

    #[test]
    fn missing_file_loads_as_defaults_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        assert!(!path.exists());
    }

    #[test]
    fn no_api_key_field_exists_in_config() {
        let text = Config::default().to_toml_string().unwrap();
        assert!(
            !text.contains("api_key ="),
            "ключ не должен храниться в файле"
        );
        assert!(text.contains("api_key_env"));
    }
}
