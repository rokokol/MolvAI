// SPDX-License-Identifier: MIT
//! Keys of cloud providers and the OS secret store: reading and masking
//!
//! The settings file holds no key and cannot hold one. It holds only the name of an environment
//! variable (`api_key_env`) and a source (`api_key_source`). The source `keyring` is the OS
//! store ([`OsKeyring`]: Secret Service on Linux, Credential Manager on Windows, Keychain on
//! macOS). If the entry is not there or the store is not available, the key is read from the
//! same environment variable, and a warning is logged instead of a silent `None`
//!
//! Everything that can reach the log is wrapped in [`ApiKey`]: its `Debug` and `Display` print
//! a mask, so "the whole struct was logged by accident" does not leak the key. The values of
//! store entries never reach the log, only their names

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing::warn;

use crate::config::{Config, LlmConfig};

/// Service name in the OS store: all molva entries live under it
pub const SERVICE: &str = "molva";

/// Name of the entry with the journal key
pub const JOURNAL_KEY_ENTRY: &str = "journal-key";

/// How many characters of the key are visible in the mask on each side
const VISIBLE_HEAD: usize = 3;
const VISIBLE_TAIL: usize = 4;

/// Error of the secret store: the value is not available, which differs from "there is none"
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("key store not available: {0}")]
    Unavailable(String),
    #[error("key store: {0}")]
    Other(String),
}

/// Secret store: a pair of "entry name — value"
///
/// `get` returns `Ok(None)` for a missing entry, and `Err` only when the store did not answer.
/// The calling code tells these cases apart in messages, but acts the same in both
pub trait SecretStore {
    fn get(&self, name: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, name: &str, value: &str) -> Result<(), SecretError>;
    fn delete(&self, name: &str) -> Result<(), SecretError>;
}

/// OS store through the `keyring` crate; the service is [`SERVICE`], the user is the entry name
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

/// Store in files: the entry `name` is the file `<directory>/<name>` with one line and
/// owner-only permissions (unix). The directory is created at the first write
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

    /// File of an entry; a name with path separators is rejected, to stay inside the directory
    pub fn path_of(&self, name: &str) -> Result<PathBuf, SecretError> {
        let valid = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && !name.contains(char::is_control);
        if !valid {
            return Err(SecretError::Other(format!("invalid entry name {name:?}")));
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

/// Store in memory, for tests and for the case "there is no store": `Debug` shows no values
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<BTreeMap<String, String>>,
    /// With `Some`, every operation answers with this error, so tests can check the failure path
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

    /// A store that always answers with an error
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
            .map_err(|_| SecretError::Other("the in-memory store is poisoned".into()))?;
        Ok(values.get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        self.check()?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| SecretError::Other("the in-memory store is poisoned".into()))?;
        values.insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        self.check()?;
        let mut values = self
            .values
            .lock()
            .map_err(|_| SecretError::Other("the in-memory store is poisoned".into()))?;
        values.remove(name);
        Ok(())
    }
}

/// A key that cannot be printed by accident
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Explicit reveal: called exactly where the key goes into a request header
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

/// Mask of the key for logs: a short key is hidden whole
pub fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= VISIBLE_HEAD + VISIBLE_TAIL {
        return "***".to_string();
    }
    let head: String = chars[..VISIBLE_HEAD].iter().collect();
    let tail: String = chars[chars.len() - VISIBLE_TAIL..].iter().collect();
    format!("{head}…{tail}")
}

/// Name of the store entry with the provider key: `llm-api-key-<provider>`
pub fn llm_key_entry(provider: &str) -> String {
    let provider = provider.trim().to_lowercase();
    let provider = if provider.is_empty() {
        "default".to_string()
    } else {
        provider
    };
    format!("llm-api-key-{provider}")
}

/// Store by source name: `keyring` is the OS store, `file` is files in
/// `<settings directory>/secrets`, and anything else is a store that always refuses (the
/// source `env` does not read it)
pub fn store_for_source(source: &str) -> Box<dyn SecretStore> {
    match source.trim().to_lowercase().as_str() {
        "keyring" => Box::new(OsKeyring),
        "file" => match Config::secrets_directory() {
            Ok(directory) => Box::new(FileStore::new(directory)),
            Err(error) => Box::new(MemoryStore::failing(&error.to_string())),
        },
        other => Box::new(MemoryStore::failing(&format!(
            "the source {other:?} does not use a store"
        ))),
    }
}

/// Key for the configured provider; `None` means there is no key, which is no error for local
/// models
pub fn api_key(cfg: &LlmConfig) -> Option<String> {
    let store = store_for_source(&cfg.api_key_source);
    api_key_with_store(cfg, store.as_ref(), |name| std::env::var(name).ok())
}

/// The same, but with an explicit source of environment variables and no OS store: the `env`
/// path, and the fallback path for `keyring` when there is no store
pub fn api_key_with<F>(cfg: &LlmConfig, lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    api_key_with_store(cfg, &MemoryStore::failing("no store connected"), lookup)
}

/// Key from the store or from an environment variable, by `api_key_source`
///
/// For `keyring` and `file`, the entry [`llm_key_entry`] in `store` is read. If it is missing or
/// the store did not answer, the variable `api_key_env` is read, with a warning that names the
/// entry but not the value
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
                    "the store entry is empty, reading the variable {}", cfg.api_key_env
                );
            }
            Ok(None) => warn!(
                entry,
                "no entry in the key store, reading the variable {}; \
                 to add one: printf '%s' \"$KEY\" | molva secret set llm",
                cfg.api_key_env
            ),
            Err(error) => warn!(
                entry,
                %error,
                "the key store did not answer, reading the variable {}",
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
        let store = MemoryStore::failing("no Secret Service");
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
            assert_eq!(mode, 0o600, "only the owner reads the key");
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
        // The fields exist only to reach `Debug`: there is no need to read them
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
