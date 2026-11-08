// SPDX-License-Identifier: MIT
//! Start of the daemon: the one process that holds the microphone and the recognizer
//!
//! The GUI stops only the daemon that it started itself: a foreign process survives Quit. File
//! transcription does not run here: it goes to the daemon over IPC, like dictation and like the
//! MCP server, so one recognizer serves every client ([`crate::commands::transcribe_file`]).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as OsCommand, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::lock;

/// Exit code of `molva daemon` when another copy already holds the socket
///
/// The CLI prints the exit code table in `molva --help`; `7` is "the daemon is already
/// running". A client that started a second copy gets this code, and the daemon that won the
/// race is the one to wait for
pub const DAEMON_ALREADY_RUNNING: i32 = 7;

/// How often a wait for the daemon knocks on the socket
pub const READY_STEP: Duration = Duration::from_millis(100);

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
    /// The daemon exited before it answered: the reason comes from its last stderr line
    #[error("the daemon exited before it answered: {reason}")]
    Startup { reason: String },
    /// The socket did not answer in time: the weights may still be downloading
    #[error("the daemon did not answer within {secs} s")]
    NotReady { secs: u64 },
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

/// How the daemon that this process started ended, if it crashed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonFailure {
    /// `None` if a signal killed the process
    pub code: Option<i32>,
    /// Last stderr line: `molva` prints the fatal error last
    pub last_line: String,
}

impl DaemonFailure {
    /// The failure in words, for a window that has to name the reason
    pub fn reason(&self) -> String {
        let code = self.code.map_or_else(
            || "killed by a signal".to_string(),
            |code| format!("exit code {code}"),
        );
        if self.last_line.is_empty() {
            code
        } else {
            format!("{code}: {}", self.last_line)
        }
    }
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
    /// Start `molva --config <path> --socket <path> daemon`
    ///
    /// The settings and the socket are named explicitly: the daemon must read the same file and
    /// listen on the same socket as the window that starts it, whatever the environment holds.
    /// A second call while the process lives does nothing
    pub fn start(&mut self, config_path: &Path, socket: &Path) -> Result<(), SidecarError> {
        if self.is_alive() {
            return Ok(());
        }
        let program = locate()?;
        let mut command = OsCommand::new(&program);
        command
            .arg("--config")
            .arg(config_path)
            .arg("--socket")
            .arg(socket)
            .arg("daemon");
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

    /// Whether the daemon that this process started has crashed, and what it said last
    ///
    /// The reader thread may not have drained the pipe yet when the process exits. So the call
    /// waits a short time for the last line: without it the window would show an exit code with
    /// no reason
    pub fn failure(&mut self) -> Option<DaemonFailure> {
        let status = self.child.as_mut()?.try_wait().ok()??;
        if status.success() {
            return None;
        }
        Some(DaemonFailure {
            code: status.code(),
            last_line: self.wait_for_last_line(),
        })
    }

    /// The last line of stderr, waiting a moment for the reader thread
    fn wait_for_last_line(&self) -> String {
        let mut last_line = None;
        for _ in 0..20 {
            last_line = lock(&self.stderr_tail).back().cloned();
            if last_line.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        last_line.unwrap_or_default()
    }

    /// Whether the process that this handle started is alive
    pub fn is_alive(&mut self) -> bool {
        match self.child.as_mut() {
            None => false,
            Some(child) => matches!(child.try_wait(), Ok(None)),
        }
    }

    /// Whether the daemon belongs to this handle: only its own Quit may stop it
    ///
    /// A child that lost the start race exits with the "already running" code, and the daemon
    /// on the socket belongs to another client: this handle owns nothing then
    pub fn is_ours(&mut self) -> bool {
        self.is_alive()
    }

    /// The process that this handle started, if it is still there
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
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

/// Wait until the daemon answers `ping`, or report why it will not
///
/// The daemon loads the recognizer before it opens the socket, and on a clean machine it also
/// downloads the weights: a start is a minute on a fast link and much longer on a slow one, so
/// the bound is generous and the message names the likely reason. A child that ended with the
/// "already running" code did not fail: another client won the race, and this wait is for its
/// daemon
pub fn wait_ready(
    ready: Duration,
    ping: &dyn Fn() -> bool,
    failure: &mut dyn FnMut() -> Option<DaemonFailure>,
) -> Result<(), SidecarError> {
    let deadline = Instant::now() + ready;
    loop {
        if ping() {
            return Ok(());
        }
        if let Some(failure) = failure() {
            if failure.code != Some(DAEMON_ALREADY_RUNNING) {
                return Err(SidecarError::Startup {
                    reason: failure.reason(),
                });
            }
        }
        if Instant::now() >= deadline {
            return Err(SidecarError::NotReady {
                secs: ready.as_secs(),
            });
        }
        std::thread::sleep(READY_STEP);
    }
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
        // A stop without a started process breaks nothing
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
            std::thread::sleep(Duration::from_millis(10));
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
        assert!(exit.reason().contains("exit code 5"), "{}", exit.reason());
        assert!(
            exit.reason().contains("network unreachable"),
            "{}",
            exit.reason()
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

    #[cfg(unix)]
    #[test]
    fn a_child_that_already_exited_is_not_owned_any_more() {
        // A second copy that lost the start race leaves this handle with a dead process: the
        // daemon on the socket belongs to the other client, and Quit must not shut it down
        let mut daemon = daemon_from_script("exit 7");
        wait_for_exit(&mut daemon);
        assert!(
            !daemon.is_ours(),
            "a dead child still claimed the daemon on the socket"
        );
    }

    #[test]
    fn a_daemon_that_answers_is_not_waited_for() {
        let started = Instant::now();
        wait_ready(Duration::from_secs(5), &|| true, &mut || None).unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_daemon_that_never_answers_names_the_bound() {
        let err = wait_ready(Duration::from_millis(150), &|| false, &mut || None).unwrap_err();
        match err {
            SidecarError::NotReady { secs } => assert_eq!(secs, 0),
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(err.to_string().contains("did not answer"), "{err}");
    }

    #[test]
    fn a_daemon_that_died_at_once_reports_its_reason_instead_of_waiting() {
        let mut calls = 0;
        let err = wait_ready(Duration::from_secs(30), &|| false, &mut || {
            calls += 1;
            Some(DaemonFailure {
                code: Some(5),
                last_line: "error: network unreachable".into(),
            })
        })
        .unwrap_err();
        assert_eq!(calls, 1, "the failure must be seen at the first check");
        assert!(err.to_string().contains("network unreachable"), "{err}");
    }

    #[test]
    fn a_second_copy_that_lost_the_race_keeps_the_wait_alive() {
        // The child ended with "already running": another client started the daemon, and this
        // wait is for that one, not a failure
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let failures = std::sync::atomic::AtomicUsize::new(0);
        let outcome = wait_ready(
            Duration::from_secs(5),
            &|| {
                // The other daemon opens the socket on the third knock
                checks.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 >= 3
            },
            &mut || {
                failures.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(DaemonFailure {
                    code: Some(DAEMON_ALREADY_RUNNING),
                    last_line: "the daemon is already running (pid 42)".into(),
                })
            },
        );
        outcome.unwrap();
        assert!(checks.load(std::sync::atomic::Ordering::SeqCst) >= 3);
        assert!(
            failures.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "the loss of the race was never seen"
        );
    }
}
