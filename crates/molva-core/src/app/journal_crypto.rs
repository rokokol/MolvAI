// SPDX-License-Identifier: MIT
//! Шифрование текстов журнала по настройке `journal.encrypt` (M-17).
//!
//! Шифруются только поля с текстом реплики: строка JSONL остаётся строкой JSONL, статистика,
//! фильтры по дате и приложению, ротация и удаление работают без ключа. Ключ — 32 случайных
//! байта в файле с правами только для владельца рядом с журналом; без ключа тексты в файле
//! нечитаемы, а история показывает пустые поля вместо шифротекста.
//!
//! Алгоритм — XChaCha20-Poly1305: случайный nonce на каждое поле, поэтому одинаковые реплики
//! дают разный шифротекст, а подмена байтов в файле не проходит проверку подлинности.

use std::io::Write as _;
use std::path::Path;

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

/// Префикс зашифрованного поля: по нему `load_all` отличает шифротекст от открытого текста
/// старых записей и не пытается расшифровать то, что никогда не шифровалось.
pub const ENCRYPTED_PREFIX: &str = "enc1:";

/// Длина ключа XChaCha20-Poly1305 в байтах.
pub const KEY_LEN: usize = 32;

const NONCE_LEN: usize = 24;

/// Ошибки ключа: файл не прочитан, не создан или содержит не ключ.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("ключ журнала {path}: {reason}")]
    Io { path: String, reason: String },
    #[error("ключ журнала {path} повреждён: ожидалось {expected} hex-символов")]
    Malformed { path: String, expected: usize },
}

/// Шифр журнала с загруженным ключом.
#[derive(Clone)]
pub struct JournalCipher {
    cipher: XChaCha20Poly1305,
}

impl std::fmt::Debug for JournalCipher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Ключ в отладочный вывод не попадает.
        formatter
            .debug_struct("JournalCipher")
            .finish_non_exhaustive()
    }
}

impl JournalCipher {
    /// Шифр из готового ключа.
    pub fn from_key(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: XChaCha20Poly1305::new(key.into()),
        }
    }

    /// Шифр из файла ключа; отсутствующий файл создаётся со случайным ключом и правами 0600.
    pub fn from_key_file(path: &Path) -> Result<Self, KeyError> {
        let key = load_or_create_key(path)?;
        Ok(Self::from_key(&key))
    }

    /// Зашифровать поле: `enc1:<hex nonce><hex шифротекст>`.
    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        // Шифрование в памяти не может не удаться: ключ и nonce правильной длины по типам.
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .unwrap_or_default();
        format!("{ENCRYPTED_PREFIX}{}{}", hex(&nonce), hex(&ciphertext))
    }

    /// Расшифровать поле. `None` — не наш формат, чужой ключ или подменённые байты.
    pub fn decrypt(&self, field: &str) -> Option<String> {
        let payload = field.strip_prefix(ENCRYPTED_PREFIX)?;
        let bytes = unhex(payload)?;
        if bytes.len() < NONCE_LEN {
            return None;
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let nonce = XNonce::from_slice(nonce);
        let plaintext = self.cipher.decrypt(nonce, ciphertext).ok()?;
        String::from_utf8(plaintext).ok()
    }
}

/// Поле уже зашифровано этим модулем.
pub fn is_encrypted(field: &str) -> bool {
    field.starts_with(ENCRYPTED_PREFIX)
}

/// Прочитать ключ из файла или создать новый. Файл — 64 hex-символа и перевод строки.
pub fn load_or_create_key(path: &Path) -> Result<[u8; KEY_LEN], KeyError> {
    let io = |error: &std::io::Error| KeyError::Io {
        path: path.display().to_string(),
        reason: error.to_string(),
    };
    if path.is_file() {
        let text = std::fs::read_to_string(path).map_err(|e| io(&e))?;
        let bytes = unhex(text.trim()).ok_or_else(|| KeyError::Malformed {
            path: path.display().to_string(),
            expected: KEY_LEN * 2,
        })?;
        return bytes.try_into().map_err(|_| KeyError::Malformed {
            path: path.display().to_string(),
            expected: KEY_LEN * 2,
        });
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| io(&e))?;
        }
    }
    let key: [u8; KEY_LEN] = XChaCha20Poly1305::generate_key(&mut OsRng).into();
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

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, byte| {
        // Запись в String не может не удаться.
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
        assert_ne!(first, second, "nonce обязан быть случайным");
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
        assert_eq!(first, second, "повторное чтение отдаёт тот же ключ");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().trim().len(),
            KEY_LEN * 2
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "ключ читает только владелец");
        }
    }

    #[test]
    fn a_damaged_key_file_is_an_explicit_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.key");
        std::fs::write(&path, "не ключ").unwrap();
        assert!(matches!(
            load_or_create_key(&path),
            Err(KeyError::Malformed { .. })
        ));
    }
}
