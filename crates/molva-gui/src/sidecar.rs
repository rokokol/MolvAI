// SPDX-License-Identifier: MIT
//! Запуск демона и разбор файлов через CLI `molva`.
//!
//! GUI останавливает только тот демон, который запустил сам: чужой процесс переживает Quit.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as OsCommand, Stdio};
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::lock;

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("исполняемый файл molva не найден: положите его рядом с GUI или в PATH")]
    NotFound,
    #[error("не удалось запустить {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`molva transcribe` завершился с кодом {code}: {stderr}")]
    Failed { code: i32, stderr: String },
    #[error("разбор отменён")]
    Cancelled,
    #[error("`molva transcribe` вернул не JSON: {0}")]
    BadOutput(String),
}

/// Где искать `molva`: рядом с GUI, потом в целевом каталоге сборки, потом в PATH.
///
/// Проверка PATH оставлена системе: `Command` сам найдёт бинарь по имени.
pub fn locate() -> Result<PathBuf, SidecarError> {
    let exe_name = if cfg!(windows) { "molva.exe" } else { "molva" };
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            let neighbour = dir.join(exe_name);
            if neighbour.is_file() {
                return Ok(neighbour);
            }
        }
    }
    Ok(PathBuf::from(exe_name))
}

/// Демон, запущенный этим процессом.
#[derive(Debug, Default)]
pub struct Daemon {
    child: Option<Child>,
}

impl Daemon {
    /// Запустить `molva daemon`. Повторный вызов при живом процессе ничего не делает.
    pub fn start(&mut self) -> Result<(), SidecarError> {
        if self.is_alive() {
            return Ok(());
        }
        let program = locate()?;
        let child = OsCommand::new(&program)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| SidecarError::Spawn {
                program: program.display().to_string(),
                source,
            })?;
        self.child = Some(child);
        Ok(())
    }

    /// Жив ли запущенный нами процесс.
    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            None => false,
            Some(child) => matches!(child.try_wait(), Ok(None)),
        }
    }

    /// Мы ли владеем демоном: только его Quit имеет право останавливать.
    pub fn is_ours(&self) -> bool {
        self.child.is_some()
    }

    /// Дождаться завершения после `Command::Shutdown`, иначе убить.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Прогресс разбора файла, как он уходит во фронтенд.
pub type ProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Запущенные разборы файлов: ключ — идентификатор задачи из фронтенда.
#[derive(Debug, Default)]
pub struct Transcriptions {
    running: Mutex<Vec<(String, Child)>>,
}

impl Transcriptions {
    /// Отменить разбор по идентификатору. `false` — такой задачи уже нет.
    pub fn cancel(&self, id: &str) -> bool {
        let mut running = lock(&self.running);
        if let Some(pos) = running.iter().position(|(key, _)| key == id) {
            let (_, mut child) = running.remove(pos);
            let _ = child.kill();
            let _ = child.wait();
            return true;
        }
        false
    }

    fn forget(&self, id: &str) {
        let mut running = lock(&self.running);
        running.retain(|(key, _)| key != id);
    }
}

/// Разобрать аудиофайл через `molva transcribe <path> --json`.
///
/// Прогресс идёт в `progress` построчно из stderr, результат — распарсенный JSON stdout.
/// Команду реализует дорожка E; до этого вызов честно сообщает, что подкоманды нет.
pub fn transcribe(
    registry: &Transcriptions,
    id: &str,
    path: &Path,
    progress: ProgressSink,
) -> Result<serde_json::Value, SidecarError> {
    let program = locate()?;
    let mut child = OsCommand::new(&program)
        .arg("transcribe")
        .arg(path)
        .arg("--json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| SidecarError::Spawn {
            program: program.display().to_string(),
            source,
        })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    {
        let mut running = lock(&registry.running);
        running.push((id.to_string(), child));
    }

    // stderr читаем в отдельном потоке: иначе полный буфер трубы остановит процесс.
    let stderr_thread = stderr.map(|stderr| {
        std::thread::spawn(move || {
            let mut collected = String::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                progress(&line);
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        })
    });

    let mut output = String::new();
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            output.push_str(&line);
            output.push('\n');
        }
    }
    let logged = stderr_thread
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();

    let status = {
        let mut running = lock(&registry.running);
        match running.iter().position(|(key, _)| key == id) {
            // Задачу вынули из реестра — значит её отменили.
            None => return Err(SidecarError::Cancelled),
            Some(pos) => {
                let (_, mut child) = running.remove(pos);
                drop(running);
                child.wait().map_err(|source| SidecarError::Spawn {
                    program: program.display().to_string(),
                    source,
                })?
            }
        }
    };
    registry.forget(id);

    if !status.success() {
        return Err(SidecarError::Failed {
            code: status.code().unwrap_or(-1),
            stderr: logged.trim().to_string(),
        });
    }
    serde_json::from_str(output.trim()).map_err(|e| SidecarError::BadOutput(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_falls_back_to_the_bare_name_for_path_lookup() {
        let found = locate().unwrap();
        let name = found.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("molva"), "{name}");
    }

    #[test]
    fn fresh_daemon_handle_owns_nothing() {
        let mut daemon = Daemon::default();
        assert!(!daemon.is_ours());
        assert!(!daemon.is_alive());
        // Остановка того, чего мы не запускали, ничего не ломает.
        daemon.stop();
    }

    #[test]
    fn cancelling_an_unknown_task_reports_that_there_was_none() {
        let registry = Transcriptions::default();
        assert!(!registry.cancel("нет такой задачи"));
    }
}
