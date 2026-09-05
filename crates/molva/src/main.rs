// SPDX-License-Identifier: MIT
//! `molva` — командная строка и демон MolvAI.
//!
//! Подкоманды добавляются дорожками по мере реализации; здесь только каркас и `config path`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use molva_core::Config;

/// Коды выхода, общие для всех подкоманд: 0 ok, 2 аргументы, 3 демон недоступен, 4 занят,
/// 5 ошибка движка, 6 ошибка файла. Константы добавляются по мере появления подкоманд.
mod exit {
    pub const OK: u8 = 0;
    pub const BAD_ARGS: u8 = 2;
    pub const FILE: u8 = 6;
}

#[derive(Parser)]
#[command(
    name = "molva",
    version,
    about = "MolvAI — открытый системный голосовой ввод"
)]
struct Cli {
    /// Путь к файлу настроек (по умолчанию ~/.config/molva/config.toml или $MOLVA_CONFIG)
    #[arg(long, global = true, env = "MOLVA_CONFIG")]
    config: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Настройки
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Показать путь к файлу настроек
    Path,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(err) => {
            eprintln!("ошибка: {err}");
            ExitCode::from(exit_code_for(&err))
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let config_path = match cli.config {
        Some(path) => path,
        None => Config::default_path()?,
    };
    match cli.command {
        Commands::Config {
            action: ConfigAction::Path,
        } => {
            println!("{}", config_path.display());
            Ok(())
        }
    }
}

/// Сопоставление ошибки с кодом выхода; неизвестные ошибки — код аргументов.
fn exit_code_for(err: &anyhow::Error) -> u8 {
    if err
        .downcast_ref::<molva_core::config::ConfigError>()
        .is_some()
    {
        return exit::FILE;
    }
    exit::BAD_ARGS
}
