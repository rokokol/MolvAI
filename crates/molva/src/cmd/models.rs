// SPDX-License-Identifier: MIT
//! `molva models` — which weights exist, where they lie and how to get them
//!
//! The download progress goes to stderr, and the data (the list, the path) goes to stdout, so that
//! `molva models path small` can be substituted into another command

use std::io::Write;
use std::path::Path;

use molva_core::app::models::{self, ModelStatus};
use molva_core::Config;

use super::{progress_enabled, CmdError};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Action {
    /// Show the model catalog and which of it is downloaded
    List {
        /// Machine output
        #[arg(long)]
        json: bool,
    },
    /// Download a model over HTTPS with a SHA-256 check
    Pull {
        /// Model name: tiny, base, small, large-v3-turbo…
        #[arg(required_unless_present = "all_small")]
        name: Option<String>,
        /// Download all models lighter than 500 MB — a set for a weak machine and for a demo
        #[arg(long, conflicts_with = "name")]
        all_small: bool,
        /// Download again, even if the file is already in place
        #[arg(long)]
        force: bool,
    },
    /// Check the checksum of an installed model
    Verify { name: String },
    /// Remove an installed model
    Remove { name: String },
    /// Print the path to the model file
    Path { name: String },
}

/// The border of a "light" model for `--all-small`: this is how much a small or quantized model weighs
pub(crate) const SMALL_MODEL_LIMIT_BYTES: u64 = 500 * 1_048_576;

/// The models that `--all-small` downloads, in order of growing size
pub(crate) fn small_models() -> Vec<&'static models::ModelInfo> {
    let mut small: Vec<_> = models::CATALOG
        .iter()
        .filter(|m| m.size_bytes <= SMALL_MODEL_LIMIT_BYTES)
        .collect();
    small.sort_by_key(|m| m.size_bytes);
    small
}

/// A size for a person: gigabytes for weights, megabytes for small things
pub(crate) fn human_size(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    const GB: f64 = 1_073_741_824.0;
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / GB)
    } else {
        format!("{:.0} MB", bytes as f64 / MB)
    }
}

/// The table of `models list`
pub(crate) fn render_list(statuses: &[ModelStatus]) -> String {
    let width = statuses
        .iter()
        .map(|s| s.info.name.len())
        .max()
        .unwrap_or(8)
        .max(6);
    let mut out = String::new();
    for status in statuses {
        let mark = if status.installed { "✓" } else { " " };
        out.push_str(&format!(
            "{mark} {:<width$}  {:>8}  {}\n",
            status.info.name,
            human_size(status.info.size_bytes),
            if status.installed {
                status.path.display().to_string()
            } else {
                format!("molva models pull {}", status.info.name)
            },
            width = width
        ));
    }
    out
}

fn dir_for(config: &Config) -> Result<std::path::PathBuf, CmdError> {
    models::models_dir(config).map_err(|e| CmdError::file(e.to_string()))
}

/// What `pull` did: downloaded, or made do with what already lies on the disk
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullReport {
    pub path: std::path::PathBuf,
    /// `false` — the model was already in place and passed the check, so the network was not needed (A-09)
    pub downloaded: bool,
}

/// Whether to download at all: `false` if the file is already in place and the hash matches
///
/// `--force` deletes what lies there and always returns `true`
pub(crate) fn needs_download(target: &Path, sha256: &str, force: bool) -> Result<bool, CmdError> {
    if force {
        if target.exists() {
            std::fs::remove_file(target)
                .map_err(|e| CmdError::file(format!("{}: {e}", target.display())))?;
        }
        return Ok(true);
    }
    let ok = models::verify(target, sha256).map_err(|e| CmdError::file(e.to_string()))?;
    Ok(!ok)
}

/// Download a model with a progress bar in stderr. The daemon downloads the weights by itself
/// the same way when they are missing (`stt.auto_pull`)
pub(crate) fn pull(
    name: &str,
    directory: &Path,
    mirror: &str,
    force: bool,
    quiet: bool,
) -> Result<PullReport, CmdError> {
    let info = models::find(name).map_err(|e| CmdError::args(e.to_string()))?;
    let target = directory.join(info.file_name);
    if !needs_download(&target, info.sha256, force)? {
        // We download nothing and say so honestly: "downloading" for a file that is already ready misleads
        // the user, especially when the model is several gigabytes
        return Ok(PullReport {
            path: target,
            downloaded: false,
        });
    }

    let bar = (!quiet).then(|| {
        let bar = indicatif::ProgressBar::new(info.size_bytes);
        if let Ok(style) =
            indicatif::ProgressStyle::with_template("{bar:32} {bytes}/{total_bytes} ({eta})")
        {
            bar.set_style(style);
        }
        bar
    });
    eprintln!("downloading {name} ({})", human_size(info.size_bytes));

    let result = models::pull(name, directory, mirror, &mut |downloaded, total| {
        if let Some(bar) = &bar {
            if total > 0 {
                bar.set_length(total);
            }
            bar.set_position(downloaded);
        }
    });
    if let Some(bar) = bar {
        bar.finish_and_clear();
    }
    result
        .map(|path| PullReport {
            path,
            downloaded: true,
        })
        .map_err(|e| CmdError::file(e.to_string()))
}

pub(crate) fn run(
    action: &Action,
    config: &Config,
    stdout: &mut dyn Write,
) -> Result<(), CmdError> {
    let directory = dir_for(config)?;
    let write = |stdout: &mut dyn Write, text: &str| -> Result<(), CmdError> {
        writeln!(stdout, "{text}").map_err(|e| CmdError::file(e.to_string()))
    };

    match action {
        Action::List { json } => {
            let statuses = models::list(&directory);
            if *json {
                let text = serde_json::to_string_pretty(&statuses)
                    .map_err(|e| CmdError::file(e.to_string()))?;
                write(stdout, &text)?;
            } else {
                eprintln!("model directory: {}", directory.display());
                write(stdout, render_list(&statuses).trim_end())?;
            }
        }
        Action::Pull {
            name,
            all_small,
            force,
        } => {
            let names: Vec<String> = if *all_small {
                small_models().iter().map(|m| m.name.to_string()).collect()
            } else {
                // clap does not let a call pass without a name and without --all-small
                name.iter().cloned().collect()
            };
            for name in &names {
                let report = pull(
                    name,
                    &directory,
                    &config.stt.models_mirror,
                    *force,
                    !progress_enabled(false),
                )?;
                if report.downloaded {
                    eprintln!("done: the checksum of {name} matches");
                } else {
                    eprintln!("{name} is already installed, the checksum matches");
                }
                write(stdout, &report.path.display().to_string())?;
            }
        }
        Action::Verify { name } => {
            let info = models::find(name).map_err(|e| CmdError::args(e.to_string()))?;
            let path = directory.join(info.file_name);
            if !path.is_file() {
                return Err(CmdError::file(format!(
                    "the model {name} is not installed. Download it: molva models pull {name}"
                )));
            }
            let ok =
                models::verify(&path, info.sha256).map_err(|e| CmdError::file(e.to_string()))?;
            if !ok {
                return Err(CmdError::file(format!(
                    "the checksum of {name} does not match: the file is damaged, \
                     install it again with the command molva models pull {name} --force"
                )));
            }
            write(stdout, &format!("{name}: SHA-256 matches"))?;
        }
        Action::Remove { name } => {
            let path =
                models::remove(name, &directory).map_err(|e| CmdError::file(e.to_string()))?;
            write(stdout, &format!("removed: {}", path.display()))?;
        }
        Action::Path { name } => {
            let path =
                models::model_path(config, name).map_err(|e| CmdError::args(e.to_string()))?;
            write(stdout, &path.display().to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_in(directory: &Path) -> Config {
        let mut config = Config::default();
        config.stt.model_path = directory.display().to_string();
        config
    }

    #[test]
    fn size_is_shown_in_megabytes_and_gigabytes() {
        assert_eq!(human_size(487_601_967), "465 MB");
        assert_eq!(human_size(3_095_033_483), "2.9 GB");
    }

    #[test]
    fn list_marks_installed_models_and_offers_a_command_for_the_rest() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("ggml-tiny.bin"), "x").unwrap();
        let text = render_list(&models::list(directory.path()));
        assert!(text.contains("✓ tiny"), "{text}");
        assert!(text.contains("molva models pull small"), "{text}");
    }

    #[test]
    fn list_json_is_machine_readable() {
        let directory = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        run(
            &Action::List { json: true },
            &cfg_in(directory.path()),
            &mut out,
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let array = value.as_array().unwrap();
        assert_eq!(array.len(), models::CATALOG.len());
        assert!(array[0].get("sha256").is_some());
        assert_eq!(array[0]["installed"], serde_json::Value::Bool(false));
    }

    #[test]
    fn path_prints_where_the_file_would_live() {
        let directory = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        run(
            &Action::Path {
                name: "small".into(),
            },
            &cfg_in(directory.path()),
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.trim().ends_with("ggml-small.bin"), "{text}");
    }

    #[test]
    fn unknown_name_is_an_argument_error() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(
            &Action::Path {
                name: "nosuchmodel".into(),
            },
            &cfg_in(directory.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.code, CmdError::BAD_ARGS);
    }

    #[test]
    fn verify_of_absent_model_suggests_pull() {
        let directory = tempfile::tempdir().unwrap();
        let error = run(
            &Action::Verify {
                name: "small".into(),
            },
            &cfg_in(directory.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(
            error.message.contains("molva models pull small"),
            "{}",
            error.message
        );
        assert_eq!(error.code, CmdError::FILE);
    }

    #[test]
    fn verify_of_corrupted_model_tells_how_to_fix_it() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("ggml-tiny.bin"), "forgery").unwrap();
        let error = run(
            &Action::Verify {
                name: "tiny".into(),
            },
            &cfg_in(directory.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(error.message.contains("--force"), "{}", error.message);
    }

    #[test]
    fn installed_and_valid_model_is_not_downloaded_again() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        std::fs::write(&path, "contents").unwrap();
        let sha = models::sha256_file(&path).unwrap();

        assert!(!needs_download(&path, &sha, false).unwrap());
        // A damaged file is downloaded again
        assert!(needs_download(&path, &"0".repeat(64), false).unwrap());
        assert!(path.exists(), "the check must not delete anything");
    }

    #[test]
    fn force_removes_the_file_and_demands_a_download() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        std::fs::write(&path, "contents").unwrap();
        let sha = models::sha256_file(&path).unwrap();

        assert!(needs_download(&path, &sha, true).unwrap());
        assert!(!path.exists(), "--force must remove the old file");
    }

    #[test]
    fn absent_model_needs_a_download() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        assert!(needs_download(&path, &"0".repeat(64), false).unwrap());
        assert!(needs_download(&path, &"0".repeat(64), true).unwrap());
    }

    #[test]
    fn all_small_covers_only_light_models_smallest_first() {
        let names: Vec<&str> = small_models().iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["tiny", "base", "small-q5_1", "small"]);
        assert!(!names.contains(&"large-v3"), "heavy models do not get here");
        for pair in small_models().windows(2) {
            assert!(pair[0].size_bytes <= pair[1].size_bytes, "order by size");
        }
    }

    #[test]
    fn remove_deletes_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        std::fs::write(&path, "x").unwrap();
        run(
            &Action::Remove {
                name: "tiny".into(),
            },
            &cfg_in(directory.path()),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(!path.exists());
    }
}
