// SPDX-License-Identifier: MIT
//! `molva` — the command line and the daemon of MolvAI
//!
//! The subcommands live in `cmd/`. This file has only the argument parsing and the exit codes

// In tests a panic is the way to report a failure, not an unhandled error. An exact
// comparison of floating-point numbers is legal (the test compares a computed value with a constant
// that the test itself set), and printing measurements to stdout is the report of a measuring test
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::print_stdout
    )
)]

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use molva_core::domain::entry::Mode;
use molva_core::infra::ipc::IpcClientError;
use molva_core::ipc::protocol::ErrorCode;
use molva_core::Config;

mod cmd;

/// Exit codes shared by all subcommands: 0 ok, 2 arguments, 3 daemon unreachable, 4 busy,
/// 5 engine error, 6 file error, 7 daemon already running
mod exit {
    pub(crate) const OK: u8 = 0;
    pub(crate) const BAD_ARGS: u8 = 2;
    pub(crate) const NO_DAEMON: u8 = 3;
    pub(crate) const BUSY: u8 = 4;
    pub(crate) const ENGINE: u8 = 5;
    pub(crate) const FILE: u8 = 6;
    /// The daemon is already running: the second copy exits with a message and does not fight for the microphone
    pub(crate) const ALREADY_RUNNING: u8 = 7;
}

#[derive(Parser)]
#[command(
    name = "molva",
    version,
    about = "MolvAI — open-source system-wide voice input"
)]
struct Cli {
    /// Path to the settings file (default ~/.config/molva/config.toml or $MOLVA_CONFIG)
    #[arg(long, global = true, env = "MOLVA_CONFIG")]
    config: Option<std::path::PathBuf>,

    /// Path to the daemon socket (default $XDG_RUNTIME_DIR/molva.sock or $MOLVA_SOCKET)
    #[arg(long, global = true)]
    socket: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Phrase history
    History(cmd::history::HistoryArgs),
    /// Dictation statistics
    Stats(cmd::stats::StatsArgs),
    /// Post-processing styles
    Styles {
        #[command(subcommand)]
        action: cmd::styles::StylesAction,
    },
    /// Term dictionary
    Dictionary {
        #[command(subcommand)]
        action: cmd::dictionary::DictionaryAction,
    },
    /// Settings
    Config {
        #[command(subcommand)]
        action: cmd::config::ConfigAction,
    },
    /// Start the daemon
    Daemon {
        /// Notifications go to the log instead of the desktop; the process does not go to the background in any case
        #[arg(long)]
        foreground: bool,
    },
    /// Recording control
    Record {
        #[command(subcommand)]
        action: RecordAction,
    },
    /// Daemon state
    Status {
        /// Print JSON instead of a line
        #[arg(long)]
        json: bool,
        /// Keep printing the events of the daemon
        #[arg(long)]
        watch: bool,
    },
    /// Transcribe an audio file, a directory or a stream from standard input
    Transcribe(cmd::transcribe::Args),
    /// Speech recognition model weights
    Models {
        #[command(subcommand)]
        action: cmd::models::Action,
    },
    /// Local checker: WER/CER and latencies on a set of audio
    Bench(cmd::bench::Args),
    /// Print the binds for the compositor
    Setup {
        /// hyprland | sway | kde | gnome
        target: String,
        /// Push-to-talk key instead of the one in the settings
        #[arg(long)]
        ptt: Option<String>,
        /// Compatibility: the output is printed anyway
        #[arg(long)]
        print: bool,
        /// Hyprland only: write the binds to ~/.config/hypr/molvai.lua and print the line that
        /// includes it
        #[arg(long)]
        write: bool,
    },
    /// Check the text paste into the active window
    TestInject {
        /// paste | type | clipboard | auto
        #[arg(long)]
        mode: Option<String>,
        /// Text instead of the default one
        #[arg(long)]
        text: Option<String>,
        /// Pause before the paste in seconds
        #[arg(long, default_value_t = cmd::test_inject::DEFAULT_DELAY.as_secs())]
        delay: u64,
    },
    /// Keys in the OS store or in a file: the value is read from stdin
    Secret {
        #[command(subcommand)]
        action: cmd::secret::SecretAction,
    },
    /// Environment diagnostics
    Doctor,
    /// Shell completion script
    Completions {
        /// bash, zsh, fish, powershell, elvish
        shell: clap_complete::Shell,
    },
    /// List of input devices
    Devices {
        #[arg(long)]
        json: bool,
    },
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
    /// Start recording
    Start {
        #[arg(long, value_enum, default_value_t = ModeArg::Dictation)]
        mode: ModeArg,
        #[arg(long)]
        style: Option<String>,
    },
    /// Stop recording and process the phrase
    Stop,
    /// Turn recording on or off
    Toggle {
        #[arg(long, value_enum, default_value_t = ModeArg::Dictation)]
        mode: ModeArg,
        #[arg(long)]
        style: Option<String>,
    },
    /// Cancel recording without creating a phrase
    Cancel,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::from(exit::OK),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(exit_code_for(&error))
        }
    }
}

fn init_logging(level: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_env("MOLVA_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    // A double initialization happens in tests and must not crash the process
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
    // The settings are read leniently: a damaged file must not stop the user from viewing the history
    let config = || -> anyhow::Result<Config> {
        let (config, warning) = Config::load_lenient(&config_path)?;
        if let Some(warning) = warning {
            eprintln!("warning: {warning}");
        }
        Ok(config)
    };

    match cli.command {
        Commands::History(args) => cmd::history::run(args, &config()?, &socket),
        Commands::Stats(args) => cmd::stats::run(args, &config()?),
        Commands::Styles { action } => cmd::styles::run(action, &config()?),
        Commands::Dictionary { action } => cmd::dictionary::run(action, &config()?, &config_path),
        Commands::Config { action } => cmd::config::run(action, &config_path, &socket),
        Commands::Daemon { foreground } => {
            let config = Config::load(&config_path)?;
            init_logging(&config.log.level);
            cmd::daemon::run(&config_path, &cmd::daemon::Options { socket, foreground })
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
        Commands::Transcribe(args) => {
            let config = Config::load(&config_path)?;
            Ok(cmd::transcribe::run(&args, &config, &config_path)?)
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
        Commands::Setup {
            target, ptt, write, ..
        } => {
            let config = Config::load(&config_path)?;
            let target = cmd::setup::Target::parse(&target).ok_or_else(|| {
                anyhow::anyhow!("unknown compositor \"{target}\": hyprland | sway | kde | gnome")
            })?;
            cmd::setup::run(target, &config.hotkeys, ptt.as_deref(), write)
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
        Commands::Secret { action } => cmd::secret::run(action, &config()?),
        Commands::Doctor => cmd::doctor::run(&socket, &config()?),
        Commands::Completions { shell } => {
            let mut stdout = std::io::stdout().lock();
            Ok(cmd::completions::run::<Cli>(shell, "molva", &mut stdout)?)
        }
        Commands::Devices { json } => cmd::devices::run(json),
    }
}

/// Maps an error to an exit code; unknown errors get the arguments code
fn exit_code_for(error: &anyhow::Error) -> u8 {
    if let Some(cmd_err) = error.downcast_ref::<cmd::CmdError>() {
        return cmd_err.code;
    }
    if error
        .downcast_ref::<molva_core::config::ConfigError>()
        .is_some()
    {
        return exit::FILE;
    }
    if let Some(ipc) = error.downcast_ref::<IpcClientError>() {
        return match ipc {
            IpcClientError::Daemon(inner) => match inner.code {
                ErrorCode::Busy => exit::BUSY,
                ErrorCode::SttFailed | ErrorCode::LlmFailed | ErrorCode::NoDevice => exit::ENGINE,
                _ => exit::BAD_ARGS,
            },
            _ => exit::NO_DAEMON,
        };
    }
    if error
        .downcast_ref::<molva_core::domain::inject::InjectError>()
        .is_some()
    {
        return exit::ENGINE;
    }
    if error
        .downcast_ref::<molva_core::domain::audio::AudioError>()
        .is_some()
    {
        return exit::ENGINE;
    }
    exit::BAD_ARGS
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use molva_core::ipc::protocol::IpcError;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn exit_codes_match_the_documented_contract() {
        assert_eq!(exit::OK, 0);
        assert_eq!(exit::BAD_ARGS, 2);
        assert_eq!(exit::NO_DAEMON, 3);
        assert_eq!(exit::BUSY, 4);
        assert_eq!(exit::ENGINE, 5);
        assert_eq!(exit::FILE, 6);
        assert_eq!(exit::ALREADY_RUNNING, 7);
        assert_eq!(cmd::CmdError::ENGINE, exit::ENGINE);
        assert_eq!(cmd::CmdError::FILE, exit::FILE);
        assert_eq!(cmd::CmdError::BAD_ARGS, exit::BAD_ARGS);
    }

    #[test]
    fn command_error_keeps_its_code_through_anyhow() {
        let error: anyhow::Error = cmd::CmdError::engine("the engine did not build").into();
        assert_eq!(exit_code_for(&error), exit::ENGINE);
        let error: anyhow::Error = cmd::CmdError::file("no such file").into();
        assert_eq!(exit_code_for(&error), exit::FILE);
    }

    #[test]
    fn a_missing_daemon_exits_with_code_three() {
        let error = anyhow::Error::from(IpcClientError::NotRunning {
            path: "/tmp/x.sock".into(),
            message: "no such file".into(),
        });
        assert_eq!(exit_code_for(&error), exit::NO_DAEMON);
    }

    #[test]
    fn a_busy_daemon_exits_with_code_four() {
        let error = anyhow::Error::from(IpcClientError::Daemon(IpcError {
            code: ErrorCode::Busy,
            message: "recording is already in progress".into(),
            hint: None,
        }));
        assert_eq!(exit_code_for(&error), exit::BUSY);
    }

    #[test]
    fn an_engine_failure_exits_with_code_five() {
        let error = anyhow::Error::from(IpcClientError::Daemon(IpcError {
            code: ErrorCode::SttFailed,
            message: "the model did not load".into(),
            hint: None,
        }));
        assert_eq!(exit_code_for(&error), exit::ENGINE);
        let inject = anyhow::Error::from(molva_core::domain::inject::InjectError::Unsupported);
        assert_eq!(exit_code_for(&inject), exit::ENGINE);
    }

    #[test]
    fn a_broken_config_exits_with_the_file_code() {
        let error = anyhow::Error::from(molva_core::config::ConfigError::NoHome);
        assert_eq!(exit_code_for(&error), exit::FILE);
    }

    #[test]
    fn an_unknown_error_falls_back_to_the_argument_code() {
        let error = anyhow::anyhow!("something went wrong");
        assert_eq!(exit_code_for(&error), exit::BAD_ARGS);
    }

    #[test]
    fn transcribe_requires_at_least_one_input() {
        assert!(Cli::try_parse_from(["molva", "transcribe"]).is_err());
        assert!(Cli::try_parse_from(["molva", "transcribe", "a.wav"]).is_ok());
    }

    #[test]
    fn models_pull_needs_a_name_or_all_small() {
        assert!(Cli::try_parse_from(["molva", "models", "pull"]).is_err());
        assert!(Cli::try_parse_from(["molva", "models", "pull", "small"]).is_ok());
        assert!(Cli::try_parse_from(["molva", "models", "pull", "--all-small"]).is_ok());
        // A name and --all-small together are a contradiction, not "both"
        assert!(Cli::try_parse_from(["molva", "models", "pull", "small", "--all-small"]).is_err());
    }

    #[test]
    fn bench_defaults_to_the_bench_directory() {
        let cli = Cli::try_parse_from(["molva", "bench"]).unwrap();
        match cli.command {
            Commands::Bench(args) => {
                assert_eq!(args.set, std::path::PathBuf::from("bench"));
                assert_eq!(args.repeat, 1);
            }
            _ => panic!("the bench subcommand was expected"),
        }
    }
}
