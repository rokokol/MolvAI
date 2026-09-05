// SPDX-License-Identifier: MIT
//! `molva` — командная строка и демон MolvAI.
//!
//! Подкоманды добавляются дорожками по мере реализации; здесь только каркас и разбор аргументов.

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use molva_core::domain::entry::Mode;
use molva_core::infra::ipc::IpcClientError;
use molva_core::ipc::protocol::ErrorCode;
use molva_core::Config;

mod cmd;

/// Коды выхода, общие для всех подкоманд: 0 ok, 2 аргументы, 3 демон недоступен, 4 занят,
/// 5 ошибка движка, 6 ошибка файла. Константы добавляются по мере появления подкоманд.
mod exit {
    pub const OK: u8 = 0;
    pub const BAD_ARGS: u8 = 2;
    pub const NO_DAEMON: u8 = 3;
    pub const BUSY: u8 = 4;
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

    /// Путь к сокету демона (по умолчанию $XDG_RUNTIME_DIR/molva.sock или $MOLVA_SOCKET)
    #[arg(long, global = true)]
    socket: Option<std::path::PathBuf>,

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
    /// Запустить демон
    Daemon {
        /// Уведомления в лог вместо рабочего стола; процесс в фон не уходит в любом случае
        #[arg(long)]
        foreground: bool,
    },
    /// Управление записью
    Record {
        #[command(subcommand)]
        action: RecordAction,
    },
    /// Состояние демона
    Status {
        /// Вывести JSON вместо строки
        #[arg(long)]
        json: bool,
        /// Продолжать печатать события демона
        #[arg(long)]
        watch: bool,
    },
    /// Напечатать бинды для композитора
    Setup {
        /// hyprland | sway | kde | gnome
        target: String,
        /// Клавиша push-to-talk вместо той, что в настройках
        #[arg(long)]
        ptt: Option<String>,
        /// Совместимость: вывод и так печатается
        #[arg(long)]
        print: bool,
    },
    /// Проверить вставку текста в активное окно
    TestInject {
        /// paste | type | clipboard | auto
        #[arg(long)]
        mode: Option<String>,
        /// Текст вместо стандартного
        #[arg(long)]
        text: Option<String>,
        /// Пауза перед вставкой в секундах
        #[arg(long, default_value_t = cmd::test_inject::DEFAULT_DELAY.as_secs())]
        delay: u64,
    },
    /// Диагностика окружения
    Doctor,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Показать путь к файлу настроек
    Path,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Dictation,
    Command,
}

impl From<ModeArg> for Mode {
    fn from(value: ModeArg) -> Mode {
        match value {
            ModeArg::Dictation => Mode::Dictation,
            ModeArg::Command => Mode::Command,
        }
    }
}

#[derive(Subcommand)]
enum RecordAction {
    /// Начать запись
    Start {
        #[arg(long, value_enum, default_value_t = ModeArg::Dictation)]
        mode: ModeArg,
        #[arg(long)]
        style: Option<String>,
    },
    /// Остановить запись и обработать реплику
    Stop,
    /// Включить или выключить запись
    Toggle {
        #[arg(long, value_enum, default_value_t = ModeArg::Dictation)]
        mode: ModeArg,
        #[arg(long)]
        style: Option<String>,
    },
    /// Отменить запись, не создавая реплику
    Cancel,
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

fn init_logging(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("MOLVA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    // Двойная инициализация случается в тестах и не должна ронять процесс.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let config_path = match cli.config {
        Some(path) => path,
        None => Config::default_path()?,
    };
    let socket = cmd::daemon::resolve_socket(cli.socket);

    match cli.command {
        Commands::Config {
            action: ConfigAction::Path,
        } => {
            println!("{}", config_path.display());
            Ok(())
        }
        Commands::Daemon { foreground } => {
            let config = Config::load(&config_path)?;
            init_logging(&config.log.level);
            cmd::daemon::run(&config_path, cmd::daemon::Options { socket, foreground })
        }
        Commands::Record { action } => {
            let (action, mode, style) = match action {
                RecordAction::Start { mode, style } => {
                    (cmd::record::Action::Start, mode.into(), style)
                }
                RecordAction::Stop => (cmd::record::Action::Stop, Mode::Dictation, None),
                RecordAction::Toggle { mode, style } => {
                    (cmd::record::Action::Toggle, mode.into(), style)
                }
                RecordAction::Cancel => (cmd::record::Action::Cancel, Mode::Dictation, None),
            };
            cmd::record::run(&socket, action, mode, style)?;
            Ok(())
        }
        Commands::Status { json, watch } => {
            cmd::status::run(&socket, json, watch)?;
            Ok(())
        }
        Commands::Setup { target, ptt, .. } => {
            let config = Config::load(&config_path)?;
            let target = cmd::setup::Target::parse(&target).ok_or_else(|| {
                anyhow::anyhow!("неизвестный композитор «{target}»: hyprland | sway | kde | gnome")
            })?;
            cmd::setup::run(target, &config.hotkeys, ptt.as_deref())
        }
        Commands::TestInject { mode, text, delay } => {
            let config = Config::load(&config_path)?;
            init_logging(&config.log.level);
            cmd::test_inject::run(
                &config,
                mode.as_deref(),
                text.as_deref(),
                std::time::Duration::from_secs(delay),
            )
        }
        Commands::Doctor => cmd::doctor::run(&socket),
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
    if let Some(ipc) = err.downcast_ref::<IpcClientError>() {
        return match ipc {
            IpcClientError::NotRunning { .. } => exit::NO_DAEMON,
            IpcClientError::Daemon(inner) => match inner.code {
                ErrorCode::Busy => exit::BUSY,
                ErrorCode::SttFailed | ErrorCode::LlmFailed | ErrorCode::NoDevice => exit::ENGINE,
                _ => exit::BAD_ARGS,
            },
            _ => exit::NO_DAEMON,
        };
    }
    if err
        .downcast_ref::<molva_core::domain::inject::InjectError>()
        .is_some()
    {
        return exit::ENGINE;
    }
    exit::BAD_ARGS
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::ipc::protocol::IpcError;

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn a_missing_daemon_exits_with_code_three() {
        let err = anyhow::Error::from(IpcClientError::NotRunning {
            path: "/tmp/x.sock".into(),
            message: "нет такого файла".into(),
        });
        assert_eq!(exit_code_for(&err), exit::NO_DAEMON);
    }

    #[test]
    fn a_busy_daemon_exits_with_code_four() {
        let err = anyhow::Error::from(IpcClientError::Daemon(IpcError {
            code: ErrorCode::Busy,
            message: "запись уже идёт".into(),
            hint: None,
        }));
        assert_eq!(exit_code_for(&err), exit::BUSY);
    }

    #[test]
    fn an_engine_failure_exits_with_code_five() {
        let err = anyhow::Error::from(IpcClientError::Daemon(IpcError {
            code: ErrorCode::SttFailed,
            message: "модель не загрузилась".into(),
            hint: None,
        }));
        assert_eq!(exit_code_for(&err), exit::ENGINE);
        let inject = anyhow::Error::from(molva_core::domain::inject::InjectError::Unsupported);
        assert_eq!(exit_code_for(&inject), exit::ENGINE);
    }

    #[test]
    fn a_broken_config_exits_with_the_file_code() {
        let err = anyhow::Error::from(molva_core::config::ConfigError::NoHome);
        assert_eq!(exit_code_for(&err), exit::FILE);
    }

    #[test]
    fn an_unknown_error_falls_back_to_the_argument_code() {
        let err = anyhow::anyhow!("что-то пошло не так");
        assert_eq!(exit_code_for(&err), exit::BAD_ARGS);
    }
}
