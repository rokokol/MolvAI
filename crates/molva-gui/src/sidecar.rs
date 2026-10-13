// SPDX-License-Identifier: MIT
//! Start of the daemon and transcription of files through the `molva` CLI
//!
//! The GUI stops only the daemon that it started itself: a foreign process survives Quit

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as OsCommand, Stdio};
use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::lock;

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("molva executable not found: put it next to the GUI or in PATH")]
    NotFound,
    #[error("cannot start {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`molva transcribe` exited with code {code}: {stderr}")]
    Failed { code: i32, stderr: String },
    #[error("transcription cancelled")]
    Cancelled,
    #[error("`molva transcribe` returned output that is not JSON: {0}")]
    BadOutput(String),
}

/// Where to look for `molva`: next to the GUI, then in the build target directory, then in PATH
///
/// The system checks PATH: `Command` finds the binary by name on its own
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

/// How the daemon that we started ended, if it crashed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonFailure {
    /// `None` if a signal killed the process
    pub code: Option<i32>,
    /// Last stderr line: `molva` prints the fatal error last
    pub last_line: String,
}

/// Remove the ANSI color codes that tracing also writes to the pipe
pub fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ parameters, then a final byte from the range @ to ~
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

/// How many of the last daemon stderr lines the process keeps in memory
const STDERR_TAIL: usize = 20;

/// The daemon that this process started
#[derive(Debug, Default)]
pub struct Daemon {
    child: Option<Child>,
    /// Tail of stderr: a separate thread reads it, otherwise a full pipe would stop the daemon
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl Daemon {
    /// Start `molva daemon`. A second call while the process lives does nothing
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

    /// Whether the daemon that we started has crashed, and what it said last
    ///
    /// The reader thread may not have drained the pipe yet when the process exits. So we wait
    /// a short time for the last line: without it the window would show an exit code with no
    /// reason
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

    /// Whether the process that we started is alive
    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            None => false,
            Some(child) => matches!(child.try_wait(), Ok(None)),
        }
    }

    /// Whether we own the daemon: only our own Quit may stop it
    pub fn is_ours(&self) -> bool {
        self.child.is_some()
    }

    /// Wait for the exit after `Command::Shutdown`, otherwise kill
    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if matches!(child.try_wait(), Ok(None)) {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// File transcription progress, as it goes to the frontend
pub type ProgressSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Running file transcriptions: the key is the job identifier from the frontend
#[derive(Debug, Default)]
pub struct Transcriptions {
    running: Mutex<Vec<(String, Child)>>,
}

impl Transcriptions {
    /// Cancel a transcription by identifier. `false` means the job no longer exists
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

/// Transcribe an audio file through `molva transcribe <path> --json`
///
/// Progress goes to `progress` line by line from stderr, and the result is the parsed JSON
/// from stdout. Track E implements the command; until then the call reports plainly that the
/// subcommand does not exist
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

    // We read stderr in a separate thread: otherwise a full pipe buffer stops the process
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
            // Someone took the job out of the registry, so it was cancelled
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
        // Stopping what we did not start breaks nothing
        daemon.stop();
    }

    #[test]
    fn colour_codes_are_stripped_from_a_log_line() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m daemon is starting"),
            "2026  INFO daemon is starting"
        );
        // Non-ASCII text must pass through unchanged
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
        panic!("the script did not finish");
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_daemon_reports_its_last_stderr_line_and_exit_code() {
        let mut daemon = daemon_from_script(
            "printf '\\033[32m INFO\\033[0m no weights, downloading\\n' >&2; \
             echo 'error: network unreachable (https://huggingface.co)' >&2; exit 5",
        );
        wait_for_exit(&mut daemon);
        let exit = daemon.failure().unwrap();
        assert_eq!(exit.code, Some(5));
        assert_eq!(
            exit.last_line,
            "error: network unreachable (https://huggingface.co)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_running_or_cleanly_stopped_daemon_has_no_failure() {
        let mut running = daemon_from_script("sleep 5");
        assert!(running.failure().is_none());
        running.stop();

        let mut clean = daemon_from_script("echo 'all good' >&2; exit 0");
        wait_for_exit(&mut clean);
        assert!(clean.failure().is_none());
    }

    #[test]
    fn cancelling_an_unknown_task_reports_that_there_was_none() {
        let registry = Transcriptions::default();
        assert!(!registry.cancel("no such job"));
    }
}
