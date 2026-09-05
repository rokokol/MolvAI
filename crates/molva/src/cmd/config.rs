// SPDX-License-Identifier: MIT
//! `molva config` — чтение, правка, проверка и перенос настроек.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use molva_core::Config;

use super::confirm;

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Показать путь к файлу настроек
    Path,
    /// Показать значение по пути `stt.model`; без пути — все настройки
    Get { key: Option<String> },
    /// Записать значение: `molva config set output.mode paste`
    Set { key: String, value: String },
    /// Открыть файл настроек в $EDITOR и проверить после правки
    Edit,
    /// Проверить файл настроек
    Validate,
    /// Вернуть значения по умолчанию
    Reset {
        #[arg(long)]
        yes: bool,
    },
    /// Выгрузить профиль настроек в файл
    Export { file: PathBuf },
    /// Загрузить профиль настроек из файла
    Import { file: PathBuf },
}

pub fn run(action: ConfigAction, config_path: &Path) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", config_path.display());
            Ok(())
        }
        ConfigAction::Get { key } => {
            let config = load(config_path)?;
            match key {
                Some(key) => println!("{}", config.get_by_path(&key)?),
                None => {
                    for key in config.keys() {
                        println!("{key} = {}", config.get_by_path(&key)?);
                    }
                }
            }
            Ok(())
        }
        ConfigAction::Set { key, value } => {
            let mut config = load(config_path)?;
            config.set_by_path(&key, &value)?;
            config.save(config_path)?;
            println!("{key} = {}", config.get_by_path(&key)?);
            println!("Демон применит настройки без пересборки: `molva config reload`.");
            Ok(())
        }
        ConfigAction::Edit => {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            // Файл должен существовать, иначе редактор откроет пустоту вместо настроек.
            Config::load_or_create(config_path)?;
            let status = std::process::Command::new(&editor)
                .arg(config_path)
                .status()?;
            if !status.success() {
                anyhow::bail!("редактор {editor} завершился с ошибкой");
            }
            validate(config_path)
        }
        ConfigAction::Validate => validate(config_path),
        ConfigAction::Reset { yes } => {
            if !confirm(
                &format!(
                    "Вернуть настройки по умолчанию ({})?",
                    config_path.display()
                ),
                yes,
            )? {
                println!("Отменено.");
                return Ok(());
            }
            Config::default().save(config_path)?;
            println!("Настройки сброшены: {}", config_path.display());
            Ok(())
        }
        ConfigAction::Export { file } => {
            load(config_path)?.export(&file)?;
            println!("Профиль сохранён: {}", file.display());
            Ok(())
        }
        ConfigAction::Import { file } => {
            let config = Config::import(&file)?;
            config.save(config_path)?;
            println!(
                "Профиль применён: {} → {}",
                file.display(),
                config_path.display()
            );
            Ok(())
        }
    }
}

/// Прочитать настройки, заменив повреждённый файл умолчаниями и сказав об этом.
fn load(path: &Path) -> anyhow::Result<Config> {
    let (config, warning) = Config::load_lenient(path)?;
    if let Some(warning) = warning {
        eprintln!("предупреждение: {warning}");
    }
    Ok(config)
}

fn validate(path: &Path) -> anyhow::Result<()> {
    let config = load(path)?;
    match config.validate() {
        Ok(()) => {
            println!("Настройки в порядке: {}", path.display());
            Ok(())
        }
        Err(issues) => {
            eprintln!("Проблемы в {}:", path.display());
            for issue in &issues {
                eprintln!("  - {issue}");
            }
            anyhow::bail!("настроек с ошибками: {}", issues.len());
        }
    }
}
