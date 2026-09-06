// SPDX-License-Identifier: MIT
//! `molva secret` — ключи в хранилище, а не в файле настроек и не в истории оболочки.
//!
//! Значение читается только из stdin: `printf '%s' "$KEY" | molva secret set llm`. В argv ключ
//! попал бы в историю оболочки и в `ps`. Где именно лежит запись, решают настройки:
//! `llm.api_key_source` для ключа модели и `journal.key_source` для ключа журнала; `status`
//! показывает только наличие записи, никогда — значение.

use std::io::Read as _;

use clap::{Subcommand, ValueEnum};
use molva_core::app::journal_crypto::KEY_LEN;
use molva_core::app::secrets::{
    llm_key_entry, FileStore, OsKeyring, SecretStore, JOURNAL_KEY_ENTRY,
};
use molva_core::Config;

use super::CmdError;

/// Какой секрет: имя записи и хранилище выводятся из настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SecretName {
    /// Ключ облачного провайдера модели (`llm.api_key_source`)
    Llm,
    /// Ключ шифрования журнала (`journal.key_source`)
    Journal,
}

impl SecretName {
    const ALL: [Self; 2] = [Self::Llm, Self::Journal];

    fn label(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Journal => "journal",
        }
    }
}

#[derive(Debug, Clone, Copy, Subcommand)]
pub(crate) enum SecretAction {
    /// Записать значение из stdin
    Set {
        /// Какой секрет: llm или journal
        name: SecretName,
    },
    /// Удалить запись
    Delete {
        /// Какой секрет: llm или journal
        name: SecretName,
    },
    /// Есть ли записи; значения не печатаются
    Status,
}

/// Куда настройки отправляют секрет.
pub(crate) enum Backend {
    /// Запись `entry` в хранилище; `place` — где оно, для отчёта.
    Store {
        store: Box<dyn SecretStore>,
        entry: String,
        place: String,
    },
    /// Только переменная окружения: записывать нечего.
    Env { variable: String },
    /// Источник `none`: ключа нет по замыслу.
    None,
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store { entry, place, .. } => formatter
                .debug_struct("Store")
                .field("entry", entry)
                .field("place", place)
                .finish_non_exhaustive(),
            Self::Env { variable } => formatter
                .debug_struct("Env")
                .field("variable", variable)
                .finish(),
            Self::None => formatter.write_str("None"),
        }
    }
}

/// Имя записи для секрета: `llm-api-key-<provider>` или `journal-key`.
pub(crate) fn entry_name(name: SecretName, provider: &str) -> String {
    match name {
        SecretName::Llm => llm_key_entry(provider),
        SecretName::Journal => JOURNAL_KEY_ENTRY.to_string(),
    }
}

/// Хранилище и имя записи по настройкам. Для журнала с `key_source = "file"` файловое
/// хранилище строится над каталогом `journal.key_path`, чтобы `molva secret` и демон читали
/// один и тот же файл.
pub(crate) fn resolve(name: SecretName, config: &Config) -> Result<Backend, CmdError> {
    let source = match name {
        SecretName::Llm => config.llm.api_key_source.trim().to_lowercase(),
        SecretName::Journal => config.journal.key_source.trim().to_lowercase(),
    };
    let entry = entry_name(name, &config.llm.provider);
    match (name, source.as_str()) {
        (_, "keyring") => Ok(Backend::Store {
            store: Box::new(OsKeyring),
            entry,
            place: "хранилище ОС".into(),
        }),
        (SecretName::Llm, "file") => {
            let directory =
                Config::secrets_directory().map_err(|e| CmdError::file(e.to_string()))?;
            Ok(Backend::Store {
                place: directory.display().to_string(),
                store: Box::new(FileStore::new(directory)),
                entry,
            })
        }
        (SecretName::Journal, "file") => {
            let path = config
                .journal_key_path()
                .map_err(|e| CmdError::file(e.to_string()))?;
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| CmdError::file(format!("{}: не файл", path.display())))?;
            let directory = path
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            Ok(Backend::Store {
                place: path.display().to_string(),
                store: Box::new(FileStore::new(directory)),
                entry: file_name,
            })
        }
        (SecretName::Llm, "env") => Ok(Backend::Env {
            variable: config.llm.api_key_env.clone(),
        }),
        (SecretName::Llm, "none") => Ok(Backend::None),
        (_, other) => Err(CmdError::args(format!(
            "источник {other:?} для секрета {} не поддерживается",
            name.label()
        ))),
    }
}

/// Ключ журнала обязан быть 64 hex-символами: иначе демон откажется открывать журнал.
fn validate(name: SecretName, value: &str) -> Result<(), CmdError> {
    if name == SecretName::Journal
        && (value.len() != KEY_LEN * 2 || !value.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err(CmdError::args(format!(
            "ключ журнала — это {} hex-символов; пустая запись создастся сама при первом \
             открытии журнала",
            KEY_LEN * 2
        )));
    }
    Ok(())
}

fn read_value_from_stdin() -> Result<String, CmdError> {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| CmdError::file(format!("stdin: {e}")))?;
    let value = text.trim_end_matches(['\r', '\n']).to_string();
    if value.trim().is_empty() {
        return Err(CmdError::args(
            "значение не задано: printf '%s' \"$KEY\" | molva secret set llm",
        ));
    }
    Ok(value)
}

pub(crate) fn run(action: SecretAction, config: &Config) -> anyhow::Result<()> {
    match action {
        SecretAction::Set { name } => {
            let backend = resolve(name, config)?;
            let Backend::Store {
                store,
                entry,
                place,
            } = backend
            else {
                return Err(refuse_env(name, &backend).into());
            };
            let value = read_value_from_stdin()?;
            validate(name, &value)?;
            store
                .set(&entry, &value)
                .map_err(|e| CmdError::file(e.to_string()))?;
            eprintln!("записано: {entry} ({place})");
            Ok(())
        }
        SecretAction::Delete { name } => {
            let backend = resolve(name, config)?;
            let Backend::Store { store, entry, .. } = backend else {
                return Err(refuse_env(name, &backend).into());
            };
            store
                .delete(&entry)
                .map_err(|e| CmdError::file(e.to_string()))?;
            eprintln!("удалено: {entry}");
            Ok(())
        }
        SecretAction::Status => {
            for name in SecretName::ALL {
                println!("{}", status_line(name, config));
            }
            Ok(())
        }
    }
}

fn refuse_env(name: SecretName, backend: &Backend) -> CmdError {
    match backend {
        Backend::Env { variable } => CmdError::args(format!(
            "источник {} — переменная окружения: задайте её сами, export {variable}=…, \
             или переключите источник на keyring/file",
            name.label()
        )),
        _ => CmdError::args(format!(
            "источник {} — none: ключ отключён настройками",
            name.label()
        )),
    }
}

/// Строка отчёта: имя, где хранится и есть ли запись. Значение не печатается.
fn status_line(name: SecretName, config: &Config) -> String {
    match resolve(name, config) {
        Ok(Backend::Store {
            store,
            entry,
            place,
        }) => match store.get(&entry) {
            Ok(Some(_)) => format!("{:<8} {entry:<24} есть    {place}", name.label()),
            Ok(None) => format!("{:<8} {entry:<24} нет     {place}", name.label()),
            Err(error) => format!("{:<8} {entry:<24} ошибка  {error}", name.label()),
        },
        Ok(Backend::Env { variable }) => {
            let set = std::env::var(&variable).is_ok_and(|v| !v.trim().is_empty());
            let state = if set {
                "задана"
            } else {
                "не задана"
            };
            format!(
                "{:<8} {variable:<24} {state}  переменная окружения",
                name.label()
            )
        }
        Ok(Backend::None) => format!("{:<8} {:<24} выключен", name.label(), "-"),
        Err(error) => format!("{:<8} {:<24} ошибка  {error}", name.label(), "-"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_name_follows_the_provider_for_llm_and_is_fixed_for_the_journal() {
        assert_eq!(entry_name(SecretName::Llm, "groq"), "llm-api-key-groq");
        assert_eq!(entry_name(SecretName::Llm, "OpenAI"), "llm-api-key-openai");
        assert_eq!(entry_name(SecretName::Journal, "groq"), "journal-key");
    }

    #[test]
    fn the_source_decides_where_the_secret_goes() {
        let mut config = Config::default();
        config.llm.provider = "groq".into();
        config.llm.api_key_source = "keyring".into();
        config.journal.key_source = "file".into();
        config.journal.key_path = "/tmp/molva-test/journal.key".into();

        match resolve(SecretName::Llm, &config).unwrap() {
            Backend::Store { entry, place, .. } => {
                assert_eq!(entry, "llm-api-key-groq");
                assert_eq!(place, "хранилище ОС");
            }
            other => panic!("{other:?}"),
        }
        match resolve(SecretName::Journal, &config).unwrap() {
            Backend::Store { entry, place, .. } => {
                assert_eq!(entry, "journal.key");
                assert_eq!(place, "/tmp/molva-test/journal.key");
            }
            other => panic!("{other:?}"),
        }

        config.llm.api_key_source = "env".into();
        config.llm.api_key_env = "GROQ_API_KEY".into();
        assert!(matches!(
            resolve(SecretName::Llm, &config).unwrap(),
            Backend::Env { variable } if variable == "GROQ_API_KEY"
        ));
        config.journal.key_source = "env".into();
        assert!(resolve(SecretName::Journal, &config).is_err());
    }

    #[test]
    fn the_journal_key_must_be_hex_of_the_right_length() {
        assert!(validate(SecretName::Journal, &"ab".repeat(KEY_LEN)).is_ok());
        assert!(validate(SecretName::Journal, "не ключ").is_err());
        assert!(validate(SecretName::Journal, &"zz".repeat(KEY_LEN)).is_err());
        assert!(validate(SecretName::Llm, "sk-anything").is_ok());
    }

    #[test]
    fn setting_an_env_sourced_secret_is_refused_with_the_variable_name() {
        let backend = Backend::Env {
            variable: "GROQ_API_KEY".into(),
        };
        let error = refuse_env(SecretName::Llm, &backend);
        assert!(error.message.contains("GROQ_API_KEY"), "{}", error.message);
        assert_eq!(error.code, CmdError::BAD_ARGS);
    }
}
