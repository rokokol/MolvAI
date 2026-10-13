// SPDX-License-Identifier: MIT
//! `molva config` — reading, editing, checking and moving the settings

use std::path::{Path, PathBuf};

use clap::Subcommand;
use molva_core::Config;

use super::confirm;

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigAction {
    /// Show the path to the settings file
    Path,
    /// Show the value at a path like `stt.model`; without a path, all settings
    Get { key: Option<String> },
    /// Write a value: `molva config set output.mode paste`
    Set { key: String, value: String },
    /// Open the settings file in $EDITOR and check it after the edit
    Edit,
    /// Check the settings file
    Validate,
    /// Restore the default values
    Reset {
        #[arg(long)]
        yes: bool,
    },
    /// Export the settings profile to a file
    Export { file: PathBuf },
    /// Load the settings profile from a file
    Import { file: PathBuf },
    /// Ask the running daemon to reread the settings file
    Reload,
}

pub(crate) fn run(action: ConfigAction, config_path: &Path, socket: &Path) -> anyhow::Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", config_path.display());
            Ok(())
        }
        ConfigAction::Reload => {
            let mut client = molva_core::infra::ipc::Client::connect(socket)?;
            client.call_ok(molva_core::ipc::protocol::Command::ConfigReload)?;
            println!("the daemon reread the settings");
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
            println!("The daemon applies the settings without a rebuild: `molva config reload`.");
            Ok(())
        }
        ConfigAction::Edit => {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            // The file must exist, otherwise the editor opens emptiness instead of the settings
            Config::load_or_create(config_path)?;
            let status = std::process::Command::new(&editor)
                .arg(config_path)
                .status()?;
            if !status.success() {
                anyhow::bail!("the editor {editor} exited with an error");
            }
            validate(config_path)
        }
        ConfigAction::Validate => validate(config_path),
        ConfigAction::Reset { yes } => {
            if !confirm(
                &format!("Restore the default settings ({})?", config_path.display()),
                yes,
            )? {
                println!("Cancelled.");
                return Ok(());
            }
            Config::default().save(config_path)?;
            println!("Settings reset: {}", config_path.display());
            Ok(())
        }
        ConfigAction::Export { file } => {
            load(config_path)?.export(&file)?;
            println!("Profile saved: {}", file.display());
            Ok(())
        }
        ConfigAction::Import { file } => {
            let config = Config::import(&file)?;
            config.save(config_path)?;
            println!(
                "Profile applied: {} → {}",
                file.display(),
                config_path.display()
            );
            Ok(())
        }
    }
}

/// Read the settings; replace a damaged file with the defaults and say so
fn load(path: &Path) -> anyhow::Result<Config> {
    let (config, warning) = Config::load_lenient(path)?;
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
    Ok(config)
}

fn validate(path: &Path) -> anyhow::Result<()> {
    let config = load(path)?;
    match config.validate() {
        Ok(()) => {
            println!("Settings are fine: {}", path.display());
            Ok(())
        }
        Err(issues) => {
            eprintln!("Problems in {}:", path.display());
            for issue in &issues {
                eprintln!("  - {issue}");
            }
            anyhow::bail!("settings with errors: {}", issues.len());
        }
    }
}
