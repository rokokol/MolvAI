// SPDX-License-Identifier: MIT
//! `molva models` — что за веса есть, где они лежат и как их получить.
//!
//! Прогресс загрузки идёт в stderr, данные (список, путь) — в stdout, чтобы
//! `molva models path small` можно было подставить в другую команду.

use std::io::Write;
use std::path::Path;

use molva_core::app::models::{self, ModelStatus};
use molva_core::Config;

use super::{progress_enabled, CmdError};

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Action {
    /// Показать каталог моделей и что из него скачано
    List {
        /// Машинный вывод
        #[arg(long)]
        json: bool,
    },
    /// Скачать модель по HTTPS с проверкой SHA-256
    Pull {
        /// Имя модели: tiny, base, small, large-v3-turbo…
        #[arg(required_unless_present = "all_small")]
        name: Option<String>,
        /// Скачать все модели легче 500 МБ — набор для слабой машины и для демо
        #[arg(long, conflicts_with = "name")]
        all_small: bool,
        /// Скачать заново, даже если файл уже на месте
        #[arg(long)]
        force: bool,
    },
    /// Проверить контрольную сумму установленной модели
    Verify { name: String },
    /// Удалить установленную модель
    Remove { name: String },
    /// Напечатать путь к файлу модели
    Path { name: String },
}

/// Граница «лёгкой» модели для `--all-small`: столько весит небольшая или квантованная модель.
pub(crate) const SMALL_MODEL_LIMIT_BYTES: u64 = 500 * 1_048_576;

/// Модели, которые скачивает `--all-small`, в порядке возрастания размера.
pub(crate) fn small_models() -> Vec<&'static models::ModelInfo> {
    let mut small: Vec<_> = models::CATALOG
        .iter()
        .filter(|m| m.size_bytes <= SMALL_MODEL_LIMIT_BYTES)
        .collect();
    small.sort_by_key(|m| m.size_bytes);
    small
}

/// Человекочитаемый размер: гигабайты для весов, мегабайты для мелочи.
pub(crate) fn human_size(bytes: u64) -> String {
    const MB: f64 = 1_048_576.0;
    const GB: f64 = 1_073_741_824.0;
    if bytes >= 1_073_741_824 {
        format!("{:.1} ГБ", bytes as f64 / GB)
    } else {
        format!("{:.0} МБ", bytes as f64 / MB)
    }
}

/// Таблица `models list`.
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

/// Что сделал `pull`: скачал или обошёлся тем, что уже лежит на диске.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullReport {
    pub path: std::path::PathBuf,
    /// `false` — модель уже была на месте и прошла проверку, сеть не понадобилась (A-09).
    pub downloaded: bool,
}

/// Нужно ли вообще качать: `false`, если файл уже на месте и хеш сходится.
///
/// `--force` удаляет то, что лежит, и всегда возвращает `true`.
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

/// Скачать модель с полоской прогресса в stderr. Тем же путём демон докачивает веса сам,
/// когда их нет (`stt.auto_pull`).
pub(crate) fn pull(
    name: &str,
    directory: &Path,
    force: bool,
    quiet: bool,
) -> Result<PullReport, CmdError> {
    let info = models::find(name).map_err(|e| CmdError::args(e.to_string()))?;
    let target = directory.join(info.file_name);
    if !needs_download(&target, info.sha256, force)? {
        // Ничего не качаем и честно об этом говорим: «скачиваю» на уже готовый файл вводит
        // в заблуждение, особенно когда моделей несколько гигабайт.
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
    eprintln!("скачиваю {name} ({})", human_size(info.size_bytes));

    let result = models::pull(name, directory, &mut |downloaded, total| {
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
                eprintln!("каталог моделей: {}", directory.display());
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
                // clap не пропустит вызов без имени и без --all-small.
                name.iter().cloned().collect()
            };
            for name in &names {
                let report = pull(name, &directory, *force, !progress_enabled(false))?;
                if report.downloaded {
                    eprintln!("готово: контрольная сумма {name} совпала");
                } else {
                    eprintln!("{name} уже установлена, контрольная сумма совпадает");
                }
                write(stdout, &report.path.display().to_string())?;
            }
        }
        Action::Verify { name } => {
            let info = models::find(name).map_err(|e| CmdError::args(e.to_string()))?;
            let path = directory.join(info.file_name);
            if !path.is_file() {
                return Err(CmdError::file(format!(
                    "модель {name} не установлена. Скачайте: molva models pull {name}"
                )));
            }
            let ok =
                models::verify(&path, info.sha256).map_err(|e| CmdError::file(e.to_string()))?;
            if !ok {
                return Err(CmdError::file(format!(
                    "контрольная сумма {name} не совпала: файл повреждён, \
                     переустановите его командой molva models pull {name} --force"
                )));
            }
            write(stdout, &format!("{name}: SHA-256 совпадает"))?;
        }
        Action::Remove { name } => {
            let path =
                models::remove(name, &directory).map_err(|e| CmdError::file(e.to_string()))?;
            write(stdout, &format!("удалено: {}", path.display()))?;
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
        assert_eq!(human_size(487_601_967), "465 МБ");
        assert_eq!(human_size(3_095_033_483), "2.9 ГБ");
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
                name: "нетакой".into(),
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
        std::fs::write(directory.path().join("ggml-tiny.bin"), "подделка").unwrap();
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
        std::fs::write(&path, "содержимое").unwrap();
        let sha = models::sha256_file(&path).unwrap();

        assert!(!needs_download(&path, &sha, false).unwrap());
        // Испорченный файл качается заново.
        assert!(needs_download(&path, &"0".repeat(64), false).unwrap());
        assert!(path.exists(), "проверка не должна ничего удалять");
    }

    #[test]
    fn force_removes_the_file_and_demands_a_download() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ggml-tiny.bin");
        std::fs::write(&path, "содержимое").unwrap();
        let sha = models::sha256_file(&path).unwrap();

        assert!(needs_download(&path, &sha, true).unwrap());
        assert!(!path.exists(), "--force должен убрать старый файл");
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
        assert!(
            !names.contains(&"large-v3"),
            "тяжёлые модели сюда не попадают"
        );
        for pair in small_models().windows(2) {
            assert!(
                pair[0].size_bytes <= pair[1].size_bytes,
                "порядок по размеру"
            );
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
