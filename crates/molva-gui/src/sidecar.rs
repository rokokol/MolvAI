// SPDX-License-Identifier: MIT
//! Запуск демона и разбор файлов через CLI `molva`.
//!
//! GUI останавливает только тот демон, который запустил сам: чужой процесс переживает Quit.

use std::collections::VecDeque;
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

/// Как завершился запущенный нами демон, если он упал.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonFailure {
    /// `None`, если процесс убит сигналом.
    pub code: Option<i32>,
    /// Последняя строка stderr: `molva` печатает фатальную ошибку последней.
    pub last_line: String,
}

/// Убрать цветовые коды ANSI, которые tracing пишет и в трубу.
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ параметры… финальный байт из диапазона @–~.
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Сколько последних строк stderr демона держится в памяти.
const STDERR_TAIL: usize = 20;

/// Демон, запущенный этим процессом.
#[derive(Debug, Default)]
pub struct Daemon {
    child: Option<Child>,
    /// Хвост stderr: его читает отдельный поток, иначе полная труба остановила бы демона.
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl Daemon {
    /// Запустить `molva daemon`. Повторный вызов при живом процессе ничего не делает.
    pub fn start(&mut self) -> Result<(), SidecarError> {
        if self.is_alive() {
            return Ok(());
        }
        let program = locate()?;
        let mut command = OsCommand::new(&program);
        command.arg("daemon");
        self.spawn(command, &program.display().to_string())
    }

    fn spawn(&mut self, mut command: OsCommand, program: &str) -> Result<(), SidecarError> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| SidecarError::Spawn {
                program: program.to_string(),
                source,
            })?;
        let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
        if let Some(stderr) = child.stderr.take() {
            let writer = Arc::clone(&tail);
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let line = strip_ansi(&line);
                    if line.trim().is_empty() {
                        continue;
                    }
                    let mut tail = lock(&writer);
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }
        self.child = Some(child);
        self.stderr_tail = tail;
        Ok(())
    }

    /// Упал ли запущенный нами демон, и что он сказал последним.
    ///
    /// Поток чтения может ещё не дочитать трубу в момент выхода процесса, поэтому последняя
    /// строка ждётся недолго: без неё окно показало бы код выхода без причины.
    pub fn failure(&mut self) -> Option<DaemonFailure> {
        let status = self.child.as_mut()?.try_wait().ok()??;
        if status.success() {
            return None;
        }
        let mut last_line = None;
        for _ in 0..20 {
            last_line = lock(&self.stderr_tail).back().cloned();
            if last_line.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Some(DaemonFailure {
            code: status.code(),
            last_line: last_line.unwrap_or_default(),
        })
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
    fn colour_codes_are_stripped_from_a_log_line() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m демон запускается"),
            "2026  INFO демон запускается"
        );
        assert_eq!(
            strip_ansi("ошибка: сеть недоступна"),
            "ошибка: сеть недоступна"
        );
    }

    #[cfg(unix)]
    fn daemon_from_script(script: &str) -> Daemon {
        let mut command = OsCommand::new("sh");
        command.arg("-c").arg(script);
        let mut daemon = Daemon::default();
        daemon.spawn(command, "sh").unwrap();
        daemon
    }

    #[cfg(unix)]
    fn wait_for_exit(daemon: &mut Daemon) {
        for _ in 0..200 {
            if !daemon.is_alive() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("скрипт не завершился");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_daemon_reports_its_last_stderr_line_and_exit_code() {
        let mut daemon = daemon_from_script(
            "printf '\\033[32m INFO\\033[0m весов нет, скачиваю\\n' >&2; \
             echo 'ошибка: сеть недоступна (https://huggingface.co)' >&2; exit 5",
        );
        wait_for_exit(&mut daemon);
        let exit = daemon.failure().unwrap();
        assert_eq!(exit.code, Some(5));
        assert_eq!(
            exit.last_line,
            "ошибка: сеть недоступна (https://huggingface.co)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_running_or_cleanly_stopped_daemon_has_no_failure() {
        let mut running = daemon_from_script("sleep 5");
        assert!(running.failure().is_none());
        running.stop();

        let mut clean = daemon_from_script("echo 'всё хорошо' >&2; exit 0");
        wait_for_exit(&mut clean);
        assert!(clean.failure().is_none());
    }

    #[test]
    fn cancelling_an_unknown_task_reports_that_there_was_none() {
        let registry = Transcriptions::default();
        assert!(!registry.cancel("нет такой задачи"));
    }
}
