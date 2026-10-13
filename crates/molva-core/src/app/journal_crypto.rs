// SPDX-License-Identifier: MIT
//! Encryption of journal texts by the `journal.encrypt` setting (M-17)
//!
//! Only the fields that hold phrase text are encrypted. A JSONL line stays a JSONL line, and
//! statistics, filters by date and application, rotation and deletion work without the key.
//! The key is 32 random bytes in a file next to the journal, readable only by the owner.
//! Without the key the texts in the file are unreadable, and the history shows empty fields
//! instead of ciphertext
//!
//! The algorithm is XChaCha20-Poly1305. Each field gets a random nonce, so equal phrases give
//! different ciphertext, and a change of bytes in the file fails the authenticity check

use std::io::Write as _;
use std::path::Path;

use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};

use super::secrets::{SecretError, SecretStore};

/// Prefix of an encrypted field: with it `load_all` tells ciphertext from the plain text of
/// old entries, and does not try to decrypt what was never encrypted
pub const ENCRYPTED_PREFIX: &str = "enc1:";

/// Length of the XChaCha20-Poly1305 key in bytes
pub const KEY_LEN: usize = 32;

const NONCE_LEN: usize = 24;

/// Key errors: the file or the store was not read, the key was not created, or the value is
/// not a key
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("journal key {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("journal key {path} is damaged: expected {expected} hex characters")]
    Malformed { path: String, expected: usize },
    #[error("journal key {entry}: {source}")]
    Store { entry: String, source: SecretError },
}

/// Journal cipher with a loaded key
#[derive(Clone)]
pub struct JournalCipher {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for JournalCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The key does not reach the debug output
        formatter
            .debug_struct("JournalCipher")
            .finish_non_exhaustive()
    }
}

impl JournalCipher {
    /// Cipher from a ready key
    pub fn from_key(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Cipher from a key file; a missing file is created with a random key and mode 0600
    pub fn from_key_file(path: &Path) -> Result<Self, KeyError> {
        let key = load_or_create_key(path)?;
        Ok(Self::from_key(&key))
    }

    /// Cipher from an entry of the secret store; a missing entry is created with a random key
    pub fn from_store(store: &dyn SecretStore, name: &str) -> Result<Self, KeyError> {
        let key = load_or_create_key_in(store, name)?;
        Ok(Self::from_key(&key))
    }

    /// Encrypt a field: `enc1:<hex nonce><hex ciphertext>`
    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = XNonce::generate();
        // Encryption in memory cannot fail: the key and the nonce have the right length by type
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .unwrap_or_default();
        format!("{ENCRYPTED_PREFIX}{}{}", hex(&nonce), hex(&ciphertext))
    }

    /// Decrypt a field. `None` means a foreign format, a foreign key or changed bytes
    pub fn decrypt(&self, field: &str) -> Option<String> {
        let payload = field.strip_prefix(ENCRYPTED_PREFIX)?;
        let bytes = unhex(payload)?;
        if bytes.len() < NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let nonce = XNonce::try_from(nonce).ok()?;
        let plaintext = self.cipher.decrypt(&nonce, ciphertext).ok()?;
        String::from_utf8(plaintext).ok()
    }
}

/// The field is already encrypted by this module
pub fn is_encrypted(field: &str) -> bool {
    field.starts_with(ENCRYPTED_PREFIX)
}

/// Read the key from a file or create a new one. The file holds 64 hex characters and a newline
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], KeyError> {
    let io = |error: &std::io::Error| KeyError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    };
    if path.is_file() {
        let text = std::fs::read_to_string(path).map_err(|e| io(&e))?;
        return parse_key(text.trim(), &path.display().to_string());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
        }
    }
    let key: [u8; KEY_LEN] = Key::generate().into();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| io(&e))?;
    writeln!(file, "{}", hex(&key)).map_err(|e| io(&e))?;
    file.sync_all().map_err(|e| io(&e))?;
    Ok(key)
}

/// Read the key from the store entry `name`, or create a new key and save it in the same entry
///
/// The value is 64 hex characters, as in the file. A damaged value is an error, not a new key,
/// because a new key would make the old journal entries unreadable without a warning
pub fn load_or_create_key_in(
    store: &dyn SecretStore,
    name: &str,
) -> Result<[u8; KEY_LEN], KeyError> {
    let store_error = |source: SecretError| KeyError::Store {
        entry: name.to_string(),
        source,
    };
    if let Some(text) = store.get(name).map_err(store_error)? {
        return parse_key(text.trim(), name);
    }
    let key: [u8; KEY_LEN] = Key::generate().into();
    store.set(name, &hex(&key)).map_err(store_error)?;
    Ok(key)
}

/// Key from a hex string; `path` tells where it came from, for the error message
fn parse_key(text: &str, path: &str) -> Result<[u8; KEY_LEN], KeyError> {
    let malformed = || KeyError::Malformed {
        path: path.to_string(),
        expected: KEY_LEN * 2,
    };
    let bytes = unhex(text).ok_or_else(malformed)?;
    bytes.try_into().map_err(|_| malformed())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, byte| {
        // A write to a String cannot fail
        let _ = write!(acc, "{byte:02x}");
        acc
    })
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> JournalCipher {
        JournalCipher::from_key(&[7u8; KEY_LEN])
    }

    #[test]
    fn a_field_survives_the_round_trip_and_is_unreadable_on_the_way() {
        let encrypted = cipher().encrypt("Собрание переносится на вторник");
        assert!(is_encrypted(&encrypted));
        assert!(!encrypted.contains("Собрание"), "{encrypted}");
        assert_eq!(
            cipher().decrypt(&encrypted).as_deref(),
            Some("Собрание переносится на вторник")
        );
    }

    #[test]
    fn the_same_text_encrypts_differently_each_time() {
        let first = cipher().encrypt("привет");
        let second = cipher().encrypt("привет");
        assert_ne!(first, second, "the nonce must be random");
    }

    #[test]
    fn a_foreign_key_or_a_tampered_byte_yields_nothing() {
        let encrypted = cipher().encrypt("секрет");
        let foreign = JournalCipher::from_key(&[9u8; KEY_LEN]);
        assert_eq!(foreign.decrypt(&encrypted), None);

        let mut tampered = encrypted.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert_eq!(cipher().decrypt(&tampered), None);
    }

    #[test]
    fn plain_text_is_not_mistaken_for_ciphertext() {
        assert!(!is_encrypted("обычный текст"));
        assert_eq!(cipher().decrypt("обычный текст"), None);
        assert_eq!(cipher().decrypt("enc1:zz"), None);
    }

    #[test]
    fn the_key_file_is_created_once_and_reused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.key");
        let first = load_or_create_key(&path).unwrap();
        let second = load_or_create_key(&path).unwrap();
        assert_eq!(first, second, "a repeated read returns the same key");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().trim().len(),
            KEY_LEN * 2
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "only the owner reads the key");
        }
    }

    #[test]
    fn the_store_key_is_created_once_and_reused() {
        use super::super::secrets::MemoryStore;
        let store = MemoryStore::new();
        let first = load_or_create_key_in(&store, "journal-key").unwrap();
        let second = load_or_create_key_in(&store, "journal-key").unwrap();
        assert_eq!(first, second, "a repeated read returns the same key");
        let stored = store.get("journal-key").unwrap().unwrap();
        assert_eq!(stored.len(), KEY_LEN * 2);
        assert_eq!(unhex(&stored).unwrap(), first.to_vec());
    }

    #[test]
    fn a_damaged_store_value_or_a_silent_store_is_an_explicit_error() {
        use super::super::secrets::MemoryStore;
        let store = MemoryStore::new();
        store.set("journal-key", "not a key").unwrap();
        assert!(matches!(
            load_or_create_key_in(&store, "journal-key"),
            Err(KeyError::Malformed { .. })
        ));
        let failing = MemoryStore::failing("no Secret Service");
        assert!(matches!(
            load_or_create_key_in(&failing, "journal-key"),
            Err(KeyError::Store { .. })
        ));
    }

    #[test]
    fn a_damaged_key_file_is_an_explicit_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.key");
        std::fs::write(&path, "not a key").unwrap();
        assert!(matches!(
            load_or_create_key(&path),
            Err(KeyError::Malformed { .. })
        ));
    }
}
