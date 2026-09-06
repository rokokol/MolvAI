// SPDX-License-Identifier: MIT
//! Конфигурация: TOML в домашнем каталоге пользователя.
//!
//! Все поля имеют значения по умолчанию, поэтому пустой файл — валидный конфиг, а частичный
//! файл дополняется. Ключи API в файле не хранятся: только имя переменной окружения или keystore.

use std::collections::BTreeMap;
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
    #[error("нет такой настройки: {0}")]
    UnknownKey(String),
    #[error("{key} = {value:?}: {message}")]
    BadValue {
        key: String,
        value: String,
        message: String,
    },
    #[error("настройки не прошли проверку:\n{}", .0.iter().map(|i| format!("  - {i}")).collect::<Vec<String>>().join("\n"))]
    Invalid(Vec<ConfigIssue>),
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
    /// Распознавать реплику кусками прямо во время записи.
    pub chunked: bool,
    /// Пауза, по которой режется кусок при потоковой обработке.
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
            // Фиксированный язык: автоопределение в whisper.cpp на CPU в пять раз медленнее
            // (23 с против 4 с на реплику в 4 с с моделью small). `auto` остаётся опцией.
            language: "ru".into(),
            allowed_languages: vec!["ru".into(), "en".into()],
            threads: 0,
            unload_after_secs: 600,
            no_speech_threshold: 0.6,
            chunked: true,
            // Пауза сегментации короче, чем `audio.vad_min_pause_ms`: там пауза решает, кончилась
            // ли реплика, а здесь — можно ли уже отправить кусок в модель.
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

/// Значение из закрытого списка; регистр не важен.
fn one_of(issues: &mut Vec<ConfigIssue>, key: &str, value: &str, allowed: &[&str]) {
    if !allowed.iter().any(|a| a.eq_ignore_ascii_case(value)) {
        issues.push(ConfigIssue::allowed(key, value, allowed));
    }
}

/// Число в закрытом диапазоне, границы включительно.
fn in_range(issues: &mut Vec<ConfigIssue>, key: &str, value: f64, lo: f64, hi: f64) {
    if value < lo || value > hi {
        issues.push(ConfigIssue::range(key, &format!("{value}"), lo, hi));
    }
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

    /// Каталог данных пользователя: журнал, модели, сохранённое аудио.
    ///
    /// `MOLVA_DATA_DIR` переопределяет — так удобно гонять несколько профилей и тесты руками.
    pub fn data_dir() -> Result<PathBuf, ConfigError> {
        if let Some(directory) = std::env::var_os("MOLVA_DATA_DIR") {
            return Ok(PathBuf::from(directory));
        }
        directories::ProjectDirs::from("", "", "molva")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .ok_or(ConfigError::NoHome)
    }

    /// Путь к журналу: из настроек или `<data_dir>/journal.jsonl`.
    pub fn journal_path(&self) -> Result<PathBuf, ConfigError> {
        if !self.journal.path.trim().is_empty() {
            return Ok(PathBuf::from(&self.journal.path));
        }
        Ok(Self::data_dir()?.join("journal.jsonl"))
    }

    /// Путь к словарю: из настроек или `dictionary.toml` рядом с файлом настроек.
    pub fn dictionary_path(&self) -> Result<PathBuf, ConfigError> {
        self.dictionary_path_near(&Self::default_path()?)
    }

    /// То же, но рядом с конкретным файлом настроек: `--config` должен уводить и словарь.
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

    /// Прочитать файл, а повреждённый — отложить в `<path>.broken` и начать с умолчаний.
    ///
    /// Возвращает настройки и предупреждение, если файл пришлось заменить: молча терять чужие
    /// настройки нельзя, но и падать на старте из-за одной лишней скобки — тоже.
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
                    "файл настроек повреждён ({message}); он сохранён как {} и заменён \
                     значениями по умолчанию",
                    broken.display()
                );
                tracing::warn!("{warning}");
                Ok((config, Some(warning)))
            }
            Err(other) => Err(other),
        }
    }

    /// Привести настройки к текущей версии схемы. `true` — что-то изменилось.
    pub fn migrate(&mut self) -> bool {
        if self.version == CONFIG_VERSION {
            return false;
        }
        if self.version > CONFIG_VERSION {
            tracing::warn!(
                version = self.version,
                supported = CONFIG_VERSION,
                "файл настроек из более новой версии MolvAI, читаю как есть"
            );
            return false;
        }
        // Версия 0 — файл, созданный до появления поля: полей не хватало, значения умолчаний
        // уже подставлены serde, остаётся отметить схему.
        self.version = CONFIG_VERSION;
        true
    }

    /// Проверить настройки целиком. Ошибки собираются все сразу, а не по одной за запуск.
    ///
    /// Порядок замечаний повторяет порядок секций в файле настроек: так их проще
    /// сопоставить с тем, что пользователь видит в редакторе.
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

    /// Общие настройки приложения.
    fn validate_general(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(issues, "ui_language", &self.ui_language, &["ru", "en"]);
    }

    /// Захват звука: усиление, длительность, громкость сигналов.
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

    /// Распознавание: движок, язык, пороги, ключ удалённого API.
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
                "ожидается `auto` или двухбуквенный код языка, например `ru`",
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
    }

    /// Доставка текста в активное окно.
    fn validate_output(&self, issues: &mut Vec<ConfigIssue>) {
        one_of(
            issues,
            "output.mode",
            &self.output.mode,
            &["auto", "paste", "type", "clipboard"],
        );
        if self.output.auto_type_max_chars == 0 {
            issues.push(ConfigIssue::new(
                "output.auto_type_max_chars",
                "0",
                "ожидается положительное число символов",
            ));
        }
    }

    /// Постобработка моделью: провайдер, ключ, лимиты, таймауты.
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
            &["keyring", "env", "none"],
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
                "постобработка включена, но адрес модели не задан",
            ));
        }
    }

    /// Стили постобработки: и умолчание, и привязки к приложениям.
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
                    "такого стиля нет: посмотрите `molva styles list`",
                ));
            }
        }
    }

    /// Оставшиеся числовые пороги, уровень логов и горячие клавиши.
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
                "клавиша удержания не задана: диктовать будет нечем",
            ));
        }
    }

    /// Все ключи настроек в виде путей через точку — для `molva config get` и подсказок.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if let Ok(value) = toml::Value::try_from(self) {
            collect_keys(&value, "", &mut keys);
        }
        keys.sort();
        keys
    }

    /// Значение по пути `stt.model`, как оно выглядело бы в TOML (строки — без кавычек).
    pub fn get_by_path(&self, path: &str) -> Result<String, ConfigError> {
        let root =
            toml::Value::try_from(self).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        let value = value_at(&root, path).ok_or_else(|| ConfigError::UnknownKey(path.into()))?;
        Ok(render(value))
    }

    /// Установить значение по пути; тип берётся из текущего значения, результат проверяется.
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

    /// Выгрузить настройки в файл — тот же формат, что и рабочий конфиг.
    pub fn export(&self, path: &Path) -> Result<(), ConfigError> {
        self.save(path)
    }

    /// Загрузить настройки из файла профиля; повреждённый или неверный файл не применяется.
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

/// Одна претензия к настройкам: где, что стоит и что ожидалось.
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
            &format!("допустимые значения: {}", allowed.join(", ")),
        )
    }

    fn range(key: &str, value: &str, lo: f64, hi: f64) -> Self {
        Self::new(key, value, &format!("ожидается число от {lo} до {hi}"))
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

/// Как значение выглядит в выводе `config get`: строка без кавычек, остальное — как в TOML.
fn render(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        toml::Value::Array(items) => items.iter().map(render).collect::<Vec<String>>().join(", "),
        other => other.to_string(),
    }
}

/// Разобрать строку из командной строки в тип текущего значения.
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
            .map_err(|_| bad("ожидается целое число")),
        toml::Value::Float(_) => raw
            .trim()
            .parse::<f64>()
            .map(toml::Value::Float)
            .map_err(|_| bad("ожидается число, например 0.4")),
        toml::Value::Boolean(_) => match raw.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "да" | "on" => Ok(toml::Value::Boolean(true)),
            "false" | "0" | "no" | "нет" | "off" => Ok(toml::Value::Boolean(false)),
            _ => Err(bad("ожидается true или false")),
        },
        toml::Value::Array(_) => Ok(toml::Value::Array(
            raw.split(',')
                .map(|item| toml::Value::String(item.trim().to_string()))
                .filter(|item| item.as_str().is_some_and(|s| !s.is_empty()))
                .collect(),
        )),
        toml::Value::Table(_) => Err(bad("это секция настроек, а не значение")),
        toml::Value::Datetime(_) => Err(bad(
            "значение такого типа менять из командной строки нельзя",
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
            "ключ не должен храниться в файле"
        );
        assert!(text.contains("api_key_env"));
    }

    #[test]
    fn defaults_pass_validation() {
        assert_eq!(Config::default().validate(), Ok(()));
    }

    #[test]
    fn streaming_is_on_out_of_the_box() {
        // Обещание продукта: черновик виден во время речи, а после отпускания остаётся хвост.
        // Выключить можно только явно в файле настроек.
        let stt = Config::default().stt;
        assert!(stt.chunked, "потоковая обработка выключена по умолчанию");
        assert!(stt.streaming_preview, "черновик выключен по умолчанию");
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
    fn a_wrong_value_names_the_key_the_value_and_what_was_expected() {
        let mut config = Config::default();
        config.output.mode = "пасте".into();
        let issues = config.validate().unwrap_err();
        assert_eq!(issues.len(), 1, "{issues:?}");
        let text = issues[0].to_string();
        assert!(text.contains("output.mode"), "{text}");
        assert!(text.contains("пасте"), "{text}");
        assert!(text.contains("auto, paste, type, clipboard"), "{text}");
    }

    #[test]
    fn every_broken_key_is_reported_at_once() {
        let mut config = Config {
            ui_language: "kz".into(),
            ..Config::default()
        };
        config.log.level = "громко".into();
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
        config.style.default = "поэма".into();
        config
            .style
            .by_app
            .insert("kitty".into(), "тоже нет".into());
        let issues = config.validate().unwrap_err();
        let keys: Vec<&str> = issues.iter().map(|i| i.key.as_str()).collect();
        assert!(keys.contains(&"style.default"), "{keys:?}");
        assert!(keys.contains(&"style.by_app.kitty"), "{keys:?}");
    }

    #[test]
    fn a_custom_style_counts_as_a_known_one() {
        let mut config = Config::default();
        config.style.custom.push(CustomStyle {
            id: "поэма".into(),
            name: "Поэма".into(),
            uses_llm: true,
            system_prompt: "Пиши стихами.".into(),
        });
        config.style.default = "поэма".into();
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
            config.get_by_path("stt.нет_такого").unwrap_err(),
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
            .set_by_path("output.auto_type_max_chars", "много")
            .unwrap_err();
        assert!(err.to_string().contains("целое число"), "{err}");
        assert_eq!(config, before);
    }

    #[test]
    fn a_value_that_fails_validation_is_refused_and_nothing_changes() {
        let mut config = Config::default();
        let before = config.clone();
        let err = config.set_by_path("log.level", "громко").unwrap_err();
        assert!(err.to_string().contains("log.level"), "{err}");
        assert_eq!(config, before);
    }

    #[test]
    fn a_section_cannot_be_assigned_a_value() {
        let mut config = Config::default();
        let err = config.set_by_path("llm", "что-нибудь").unwrap_err();
        assert!(err.to_string().contains("секция"), "{err}");
    }

    #[test]
    fn keys_list_every_leaf_and_no_sections() {
        let keys = Config::default().keys();
        assert!(keys.contains(&"stt.model".to_string()));
        assert!(keys.contains(&"llm.timeout_secs".to_string()));
        assert!(keys.contains(&"privacy.no_record_mode".to_string()));
        assert!(!keys.contains(&"llm".to_string()));
        // Список отсортирован, чтобы вывод `config get` был стабилен.
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
        let warning = warning.expect("пользователь должен узнать о подмене");
        assert!(warning.contains("повреждён"), "{warning}");

        let broken = directory.path().join("config.toml.broken");
        assert!(broken.exists(), "испорченный файл должен сохраниться");
        assert!(std::fs::read_to_string(&broken)
            .unwrap()
            .contains("= = true"));
        // На месте настроек теперь рабочий файл.
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
        // Повторная миграция уже ничего не делает.
        assert!(!config.migrate());
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
        std::fs::write(&path, "[output]\nmode = \"телепатия\"\n").unwrap();
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
