// SPDX-License-Identifier: MIT
//! Ключи облачных провайдеров и хранилище секретов ОС: чтение и маскирование.
//!
//! В файле настроек ключа нет и быть не может — там лежит только имя переменной окружения
//! (`api_key_env`) и источник (`api_key_source`). Источник `keyring` — хранилище ОС
//! ([`OsKeyring`]: Secret Service на Linux, Credential Manager на Windows, Keychain на macOS);
//! если записи там нет или хранилище недоступно, ключ читается из той же переменной окружения
//! и об этом пишется предупреждение, а не тихо возвращается `None`.
//!
//! Всё, что может попасть в лог, оборачивается в [`ApiKey`]: его `Debug` и `Display` печатают
//! маску, поэтому «случайно залогировали структуру целиком» не приводит к утечке. Значения
//! записей хранилища в лог не попадают никогда — только их имена.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::warn;

use crate::config::{Config, LlmConfig};

/// Имя службы в хранилище ОС: под ним лежат все записи molva.
pub const SERVICE: &str = "molva";

/// Имя записи с ключом журнала.
pub const JOURNAL_KEY_ENTRY: &str = "journal-key";

/// Сколько символов ключа видно в маске с каждой стороны.
const VISIBLE_HEAD: usize = 3;
const VISIBLE_TAIL: usize = 4;

/// Ошибка хранилища секретов: значение недоступно, а не «его нет».
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("хранилище ключей недоступно: {0}")]
    Unavailable(String),
    #[error("хранилище ключей: {0}")]
    Other(String),
}

/// Хранилище секретов: пара «имя записи — значение».
///
/// `get` отдаёт `Ok(None)` для отсутствующей записи и `Err` только когда хранилище не
/// ответило; вызывающий код различает эти случаи в сообщениях, но в обоих ведёт себя одинаково.
pub trait SecretStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError>;
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

/// Хранилище ОС через крейт `keyring`; служба — [`SERVICE`], пользователь — имя записи.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeyring;

impl OsKeyring {
    fn entry(name: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(SERVICE, name).map_err(|error| convert(&error))
    }
}

fn convert(error: &keyring::Error) -> SecretError {
    match error {
        keyring::Error::NoStorageAccess(reason) | keyring::Error::PlatformFailure(reason) => {
            SecretError::Unavailable(reason.to_string())
        }
        other => SecretError::Other(other.to_string()),
    }
}

impl SecretStore for OsKeyring {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        match Self::entry(name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(convert(&error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        Self::entry(name)?
            .set_password(value)
            .map_err(|error| convert(&error))
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        match Self::entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(convert(&error)),
        }
    }
}

/// Хранилище в файлах: запись `name` — файл `<directory>/<name>` из одной строки с правами
/// только для владельца (unix). Каталог создаётся при первой записи.
#[derive(Debug, Clone)]
pub struct FileStore {
    directory: PathBuf,
}

impl FileStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Файл записи; имя с разделителями пути не принимается, чтобы не выйти из каталога.
    pub fn path_of(&self, name: &str) -> Result<PathBuf, SecretError> {
        let valid = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && !name.contains(char::is_control);
        if !valid {
            return Err(SecretError::Other(format!(
                "недопустимое имя записи {name:?}"
            )));
        }
        Ok(self.directory.join(name))
    }
}

fn io_error(path: &Path, error: &std::io::Error) -> SecretError {
    SecretError::Other(format!("{}: {error}", path.display()))
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        let path = self.path_of(name)?;
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text.trim_end_matches(['\r', '\n']).to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(&path, &error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        use std::io::Write as _;
        let path = self.path_of(name)?;
        std::fs::create_dir_all(&self.directory).map_err(|e| io_error(&self.directory, &e))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&path).map_err(|e| io_error(&path, &e))?;
        writeln!(file, "{value}").map_err(|e| io_error(&path, &e))?;
        file.sync_all().map_err(|e| io_error(&path, &e))
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        let path = self.path_of(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&path, &error)),
        }
    }
}

/// Хранилище в памяти для тестов и для «хранилища нет»: `Debug` не показывает значений.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<BTreeMap<String, String>>,
    /// `Some` — каждая операция отвечает этой ошибкой: так тесты проверяют путь отказа.
    failure: Option<String>,
}

impl fmt::Debug for MemoryStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<String> = self
            .values
            .lock()
            .map(|values| values.keys().cloned().collect())
            .unwrap_or_default();
        formatter
            .debug_struct("MemoryStore")
            .field("names", &names)
            .field("failure", &self.failure)
            .finish()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Хранилище, которое всегда отвечает ошибкой.
    pub fn failing(reason: &str) -> Self {
        Self {
            values: Mutex::new(BTreeMap::new()),
            failure: Some(reason.to_string()),
        }
    }

    fn check(&self) -> Result<(), SecretError> {
        match &self.failure {
            Some(reason) => Err(SecretError::Unavailable(reason.clone())),
            None => Ok(()),
        }
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError> {
        self.check()?;
        let values = self
            .values
            .lock()
            .map_err(|_| SecretError::Other("хранилище в памяти отравлено".into()))?;
        Ok(values.get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        self.check()?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| SecretError::Other("хранилище в памяти отравлено".into()))?;
        values.insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.check()?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| SecretError::Other("хранилище в памяти отравлено".into()))?;
        values.remove(name);
        Ok(())
    }
}

/// Ключ, который нельзя случайно напечатать.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Явное раскрытие: вызывается ровно там, где ключ уходит в заголовок запроса.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn masked(&self) -> String {
        mask(&self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiKey({})", self.masked())
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.masked())
    }
}

/// Маска ключа для логов: короткий ключ скрывается целиком.
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= VISIBLE_HEAD + VISIBLE_TAIL {
        return "***".to_string();
    }
    let head: String = chars[..VISIBLE_HEAD].iter().collect();
    let tail: String = chars[chars.len() - VISIBLE_TAIL..].iter().collect();
    format!("{head}…{tail}")
}

/// Имя записи хранилища с ключом провайдера: `llm-api-key-<provider>`.
pub fn llm_key_entry(provider: &str) -> String {
    let provider = provider.trim().to_lowercase();
    let provider = if provider.is_empty() {
        "default".to_string()
    } else {
        provider
    };
    format!("llm-api-key-{provider}")
}

/// Хранилище по имени источника: `keyring` — ОС, `file` — файлы в `<каталог настроек>/secrets`,
/// остальное — хранилище, которое всегда отвечает отказом (источник `env` его не читает).
pub fn store_for_source(source: &str) -> Box<dyn SecretStore> {
    match source.trim().to_lowercase().as_str() {
        "keyring" => Box::new(OsKeyring),
        "file" => match Config::secrets_directory() {
            Ok(directory) => Box::new(FileStore::new(directory)),
            Err(error) => Box::new(MemoryStore::failing(&error.to_string())),
        },
        other => Box::new(MemoryStore::failing(&format!(
            "источник {other:?} хранилища не использует"
        ))),
    }
}

/// Ключ для настроенного провайдера; `None` — ключа нет, и это не ошибка для локальных моделей.
pub fn api_key(cfg: &LlmConfig) -> Option<String> {
    let store = store_for_source(&cfg.api_key_source);
    api_key_with_store(cfg, store.as_ref(), |name| std::env::var(name).ok())
}

/// То же, но с явным источником переменных окружения и без хранилища ОС: путь `env`
/// и запасной путь для `keyring`, когда хранилища нет.
pub fn api_key_with<F>(cfg: &LlmConfig, lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    api_key_with_store(
        cfg,
        &MemoryStore::failing("хранилище не подключено"),
        lookup,
    )
}

/// Ключ из хранилища или переменной окружения по `api_key_source`.
///
/// `keyring` и `file`: запись [`llm_key_entry`] в `store`; если её нет или хранилище не
/// ответило, читается переменная `api_key_env` с предупреждением, где названа запись,
/// но не значение.
pub fn api_key_with_store<F>(cfg: &LlmConfig, store: &dyn SecretStore, lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let source = cfg.api_key_source.trim().to_lowercase();
    if source == "none" {
        return None;
    }
    if source == "keyring" || source == "file" {
        let entry = llm_key_entry(&cfg.provider);
        match store.get(&entry) {
            Ok(Some(value)) => {
                if let Some(value) = non_blank(&value) {
                    return Some(value);
                }
                warn!(
                    entry,
                    "запись хранилища пуста, читаю переменную {}", cfg.api_key_env
                );
            }
            Ok(None) => warn!(
                entry,
                "записи в хранилище ключей нет, читаю переменную {}; \
                 добавить: printf '%s' \"$KEY\" | molva secret set llm",
                cfg.api_key_env
            ),
            Err(error) => warn!(
                entry,
                %error,
                "хранилище ключей не ответило, читаю переменную {}",
                cfg.api_key_env
            ),
        }
    }
    let name = cfg.api_key_env.trim();
    if name.is_empty() {
        return None;
    }
    non_blank(&lookup(name)?)
}

fn non_blank(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(source: &str, env: &str) -> LlmConfig {
        LlmConfig {
            api_key_source: source.into(),
            api_key_env: env.into(),
            provider: "groq".into(),
            ..LlmConfig::default()
        }
    }

    fn env_with(value: &str) -> impl Fn(&str) -> Option<String> {
        let value = value.to_string();
        move |name| (name == "MOLVA_TEST_KEY").then(|| value.clone())
    }

    #[test]
    fn the_key_is_read_from_the_named_environment_variable() {
        let key = api_key_with(
            &cfg("env", "MOLVA_TEST_KEY"),
            env_with("sk-secret-value-1234"),
        );
        assert_eq!(key.as_deref(), Some("sk-secret-value-1234"));
    }

    #[test]
    fn keyring_is_preferred_over_the_environment() {
        let store = MemoryStore::new();
        store
            .set("llm-api-key-groq", "sk-from-keyring-0001")
            .unwrap();
        let key = api_key_with_store(
            &cfg("keyring", "MOLVA_TEST_KEY"),
            &store,
            env_with("sk-from-env-0002"),
        );
        assert_eq!(key.as_deref(), Some("sk-from-keyring-0001"));
    }

    #[test]
    fn the_entry_name_follows_the_provider() {
        let store = MemoryStore::new();
        store.set("llm-api-key-openai", "sk-openai-0003").unwrap();
        let mut config = cfg("keyring", "MOLVA_TEST_KEY");
        config.provider = "OpenAI".into();
        let key = api_key_with_store(&config, &store, |_| None);
        assert_eq!(key.as_deref(), Some("sk-openai-0003"));
        assert_eq!(llm_key_entry(" Groq "), "llm-api-key-groq");
        assert_eq!(llm_key_entry(""), "llm-api-key-default");
    }

    #[test]
    fn a_missing_entry_falls_back_to_the_environment() {
        let store = MemoryStore::new();
        let key = api_key_with_store(
            &cfg("keyring", "MOLVA_TEST_KEY"),
            &store,
            env_with("sk-from-env-0002"),
        );
        assert_eq!(key.as_deref(), Some("sk-from-env-0002"));
    }

    #[test]
    fn a_store_error_falls_back_to_the_environment() {
        let store = MemoryStore::failing("нет Secret Service");
        let key = api_key_with_store(
            &cfg("keyring", "MOLVA_TEST_KEY"),
            &store,
            env_with("sk-from-env-0002"),
        );
        assert_eq!(key.as_deref(), Some("sk-from-env-0002"));
    }

    #[test]
    fn the_env_source_ignores_the_store() {
        let store = MemoryStore::new();
        store
            .set("llm-api-key-groq", "sk-from-keyring-0001")
            .unwrap();
        let key = api_key_with_store(
            &cfg("env", "MOLVA_TEST_KEY"),
            &store,
            env_with("sk-from-env-0002"),
        );
        assert_eq!(key.as_deref(), Some("sk-from-env-0002"));
    }

    #[test]
    fn keyring_falls_back_to_the_same_variable_instead_of_failing() {
        let key = api_key_with(&cfg("keyring", "MOLVA_TEST_KEY"), |_| {
            Some("sk-secret-value-1234".into())
        });
        assert_eq!(key.as_deref(), Some("sk-secret-value-1234"));
    }

    #[test]
    fn an_absent_blank_or_disabled_source_gives_no_key() {
        assert_eq!(api_key_with(&cfg("env", "MOLVA_TEST_KEY"), |_| None), None);
        assert_eq!(
            api_key_with(&cfg("env", "MOLVA_TEST_KEY"), |_| Some("   ".into())),
            None
        );
        assert_eq!(api_key_with(&cfg("env", ""), |_| Some("x".into())), None);
        assert_eq!(
            api_key_with(&cfg("none", "MOLVA_TEST_KEY"), |_| Some("x".into())),
            None
        );
    }

    #[test]
    fn the_memory_store_round_trips_and_forgets() {
        let store = MemoryStore::new();
        assert_eq!(store.get("journal-key").unwrap(), None);
        store.set("journal-key", "abc").unwrap();
        assert_eq!(store.get("journal-key").unwrap().as_deref(), Some("abc"));
        store.delete("journal-key").unwrap();
        assert_eq!(store.get("journal-key").unwrap(), None);
        let printed = format!("{store:?}");
        assert!(!printed.contains("abc"), "{printed}");
    }

    #[test]
    fn the_file_store_keeps_one_file_per_entry_for_the_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path().join("secrets"));
        assert_eq!(store.get("llm-api-key-groq").unwrap(), None);
        store.set("llm-api-key-groq", "sk-file-0004").unwrap();
        assert_eq!(
            store.get("llm-api-key-groq").unwrap().as_deref(),
            Some("sk-file-0004")
        );
        let path = directory.path().join("secrets").join("llm-api-key-groq");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sk-file-0004\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "ключ читает только владелец");
        }
        store.delete("llm-api-key-groq").unwrap();
        assert!(!path.exists());
        store.delete("llm-api-key-groq").unwrap();
        assert!(store.path_of("../etc").is_err());
        assert!(store.path_of("").is_err());
    }

    #[test]
    fn the_file_source_reads_the_store_like_keyring() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileStore::new(directory.path());
        store.set("llm-api-key-groq", "sk-file-0004").unwrap();
        let key = api_key_with_store(
            &cfg("file", "MOLVA_TEST_KEY"),
            &store,
            env_with("sk-from-env-0002"),
        );
        assert_eq!(key.as_deref(), Some("sk-file-0004"));
    }

    #[test]
    fn a_masked_key_never_contains_the_key() {
        let secret = "sk-proj-0123456789abcdef";
        let masked = mask(secret);
        assert!(!masked.contains(secret), "{masked}");
        assert!(!secret.contains(&masked), "{masked}");
        assert_eq!(masked, "sk-…cdef");
    }

    #[test]
    fn a_short_key_is_hidden_completely() {
        assert_eq!(mask("abc"), "***");
        assert_eq!(mask(""), "***");
        assert_eq!(mask("1234567"), "***");
    }

    #[test]
    fn printing_the_key_prints_the_mask() {
        let secret = "sk-proj-0123456789abcdef";
        let key = ApiKey::new(secret);
        let printed = format!("{key} {key:?}");
        assert!(!printed.contains(secret), "{printed}");
        assert!(!printed.contains("0123456789"), "{printed}");
        assert!(printed.contains("sk-…cdef"), "{printed}");
        assert_eq!(key.expose(), secret);
    }

    #[test]
    fn a_log_line_with_the_key_inside_a_struct_stays_masked() {
        // Поля существуют ровно для того, чтобы попасть в `Debug`: читать их незачем.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Client {
            base_url: String,
            api_key: Option<ApiKey>,
        }
        let secret = "sk-proj-0123456789abcdef";
        let client = Client {
            base_url: "https://api.groq.com/openai/v1".into(),
            api_key: Some(ApiKey::new(secret)),
        };
        let line = format!("{client:?}");
        assert!(!line.contains(secret), "{line}");
        assert!(line.contains("api.groq.com"), "{line}");
    }
}
