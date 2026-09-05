// SPDX-License-Identifier: MIT
//! `molva` — командная строка и демон MolvAI.
//!
//! Подкоманды добавляются дорожками по мере реализации; здесь только каркас и `config path`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use molva_core::Config;

mod cmd;

/// Коды выхода, общие для всех подкоманд: 0 ok, 2 аргументы, 3 демон недоступен, 4 занят,
/// 5 ошибка движка, 6 ошибка файла. Константы добавляются по мере появления подкоманд.
mod exit {
    pub const OK: u8 = 0;
    pub const BAD_ARGS: u8 = 2;
    pub const ENGINE: u8 = 5;
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
    /// Расшифровать аудиофайл, каталог или поток со стандартного ввода
    Transcribe(cmd::transcribe::Args),
    /// Веса моделей распознавания
    Models {
        #[command(subcommand)]
        action: cmd::models::Action,
    },
    /// Локальный чекер: WER/CER и задержки на наборе аудио
    Bench(cmd::bench::Args),
    /// Скрипт автодополнения для оболочки
    Completions {
        /// bash, zsh, fish, powershell, elvish
        shell: clap_complete::Shell,
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
        Commands::Transcribe(args) => {
            let config = Config::load(&config_path)?;
            Ok(cmd::transcribe::run(&args, &config)?)
        }
        Commands::Models { action } => {
            let config = Config::load(&config_path)?;
            let mut stdout = std::io::stdout().lock();
            Ok(cmd::models::run(&action, &config, &mut stdout)?)
        }
        Commands::Bench(args) => {
            let config = Config::load(&config_path)?;
            let mut stdout = std::io::stdout().lock();
            Ok(cmd::bench::run(&args, &config, &mut stdout)?)
        }
        Commands::Completions { shell } => {
            let mut stdout = std::io::stdout().lock();
            Ok(cmd::completions::run::<Cli>(shell, "molva", &mut stdout)?)
        }
    }
}

/// Сопоставление ошибки с кодом выхода; неизвестные ошибки — код аргументов.
fn exit_code_for(err: &anyhow::Error) -> u8 {
    if let Some(cmd_err) = err.downcast_ref::<cmd::CmdError>() {
        return cmd_err.code;
    }
    if err
        .downcast_ref::<molva_core::config::ConfigError>()
        .is_some()
    {
        return exit::FILE;
    }
    exit::BAD_ARGS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn exit_codes_match_the_documented_contract() {
        assert_eq!(exit::OK, 0);
        assert_eq!(exit::BAD_ARGS, 2);
        assert_eq!(exit::ENGINE, 5);
        assert_eq!(exit::FILE, 6);
        assert_eq!(cmd::CmdError::ENGINE, exit::ENGINE);
        assert_eq!(cmd::CmdError::FILE, exit::FILE);
        assert_eq!(cmd::CmdError::BAD_ARGS, exit::BAD_ARGS);
    }

    #[test]
    fn command_error_keeps_its_code_through_anyhow() {
        let err: anyhow::Error = cmd::CmdError::engine("движок не собрался").into();
        assert_eq!(exit_code_for(&err), exit::ENGINE);
        let err: anyhow::Error = cmd::CmdError::file("нет файла").into();
        assert_eq!(exit_code_for(&err), exit::FILE);
    }

    #[test]
    fn transcribe_requires_at_least_one_input() {
        assert!(Cli::try_parse_from(["molva", "transcribe"]).is_err());
        assert!(Cli::try_parse_from(["molva", "transcribe", "a.wav"]).is_ok());
    }

    #[test]
    fn bench_defaults_to_the_bench_directory() {
        let cli = Cli::try_parse_from(["molva", "bench"]).unwrap();
        match cli.command {
            Commands::Bench(args) => {
                assert_eq!(args.set, std::path::PathBuf::from("bench"));
                assert_eq!(args.repeat, 1);
            }
            _ => panic!("ожидалась подкоманда bench"),
        }
    }
}
