// SPDX-License-Identifier: MIT
//! A job that a client may give up on, and the child process it is waiting for
//!
//! Waiting for a flag between two steps is not enough for a file of hours: the model works on
//! the whole of it in one call, and the wait would last as long as the recognition. The token
//! holds the child process itself, so a cancel reaches the process: the model dies, the call
//! returns at once, and nothing of the job reaches the journal
//!
//! The process is owned, never named. A process identifier outlives the process that had it: the
//! kernel hands the number to whichever process asks next, and a signal meant for a job that has
//! already finished would reach a stranger. A handle keeps the process named for as long as this
//! token holds it, and killing a process that was already collected does nothing

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use tracing::debug;

use shared_child::SharedChild;

/// The flag and the process of one cancellable job, guarded as one value
///
/// One mutex covers both, so a cancel cannot slip between the two steps of a registration and
/// leave a killed job registered, and a registration cannot arrive after the kill and stay
/// unkilled
#[derive(Debug, Default)]
struct State {
    /// The client has given up. It goes up before any kill, so a step that starts after the call
    /// sees it
    cancelled: bool,
    /// The process that does the work; `None` means that no job registered one
    child: Option<Arc<SharedChild>>,
}

/// Shared flag and process of one cancellable job
///
/// The mutex guards the flag and the registration, never the work: it is not held while a caller
/// waits for its process, reads its output or joins a thread
#[derive(Debug, Default)]
pub struct CancelToken {
    state: Mutex<State>,
    /// Signalled when the flag goes up or a process is registered
    changed: Condvar,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the client has given up
    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Give up: the flag goes up first, so a step that starts after this call sees it, and then
    /// the registered process is killed
    pub fn cancel(&self) {
        let mut state = self.lock();
        state.cancelled = true;
        self.changed.notify_all();
        if let Some(child) = state.child.as_ref() {
            kill(child);
        }
    }

    /// Take the process that does the work, so a cancel can reach it
    ///
    /// A cancel that arrived before this call had no process to kill yet, so it kills here: the
    /// registration is the first moment the kill is possible, and it cannot be missed
    pub fn attach(&self, child: Arc<SharedChild>) {
        let mut state = self.lock();
        if state.cancelled {
            kill(&child);
        }
        state.child = Some(child);
        self.changed.notify_all();
    }

    /// The process is gone, however it ended: its registration goes, so a later cancel has
    /// nothing to kill
    ///
    /// Only a registration of this very process is cleared. A late call of a finished job cannot
    /// take the registration of the job that runs now, which is what a plain `detach` would do
    pub fn finish(&self, child: &Arc<SharedChild>) {
        let mut state = self.lock();
        let mine = state
            .child
            .as_ref()
            .is_some_and(|registered| Arc::ptr_eq(registered, child));
        if mine {
            state.child = None;
            self.changed.notify_all();
        }
    }

    /// Whether a process is registered at this moment
    #[cfg(test)]
    pub(crate) fn is_attached(&self) -> bool {
        self.lock().child.is_some()
    }

    /// Wait until a process is registered, or the client gives up
    ///
    /// A test needs this to place a cancel after the start of a process without a sleep of a
    /// guessed length: the answer says which of the two happened first
    #[cfg(test)]
    pub(crate) fn wait_attached(&self, wait: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + wait;
        let mut state = self.lock();
        loop {
            if state.child.is_some() {
                return true;
            }
            if state.cancelled {
                return false;
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Kill one process, on every platform the crate builds for
///
/// A process that has already exited and was collected makes this a no-op, because the handle
/// cannot name another process. A failure is not worth an error: the job is over on both sides,
/// and the caller learns the outcome when it collects its own process
fn kill(child: &SharedChild) {
    if let Err(err) = child.kill() {
        debug!(error = %err, "the process of a cancelled job was already gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::time::Duration;

    /// The longest a test waits for a process that a signal should have ended
    const ENDED: Duration = Duration::from_secs(10);

    /// A helper process that outlives the test by far, so a kill is the only way it ends soon
    #[cfg(unix)]
    fn sleeping_child() -> Arc<SharedChild> {
        let mut command = Command::new("sh");
        command.arg("-c").arg("sleep 300");
        Arc::new(SharedChild::spawn(&mut command).expect("the helper process starts"))
    }

    /// Whether the process ended, and with what status; `None` means that it still runs
    #[cfg(unix)]
    fn ended(child: &SharedChild) -> Option<std::process::ExitStatus> {
        child
            .wait_timeout(ENDED)
            .expect("the process is ours to collect")
    }

    fn stop(child: &SharedChild) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_new_token_is_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        assert!(!token.is_attached());
    }

    #[test]
    fn a_cancel_is_visible_after_the_call() {
        let token = CancelToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_kills_the_registered_process() {
        let token = CancelToken::new();
        let child = sleeping_child();
        token.attach(Arc::clone(&child));
        assert!(token.is_attached());

        token.cancel();

        let status = ended(&child).expect("the cancel reached the process");
        assert!(
            !status.success(),
            "the process ended on its own, not from the cancel"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_before_the_registration_kills_the_process_at_the_registration() {
        let token = CancelToken::new();
        token.cancel();
        let child = sleeping_child();

        token.attach(Arc::clone(&child));

        let status = ended(&child).expect("the process was killed when it was registered");
        assert!(!status.success());
    }

    #[cfg(unix)]
    #[test]
    fn a_finish_of_a_stranger_leaves_the_registration_in_place() {
        let token = CancelToken::new();
        let mine = sleeping_child();
        let stranger = sleeping_child();
        token.attach(Arc::clone(&mine));

        token.finish(&stranger);
        token.cancel();

        let status = ended(&mine).expect("the registration of the job itself was still there");
        assert!(!status.success());
        assert!(
            stranger
                .wait_timeout(Duration::from_millis(50))
                .expect("the stranger is ours to poll")
                .is_none(),
            "the cancel reached a process that was never registered"
        );
        stop(&stranger);
    }

    #[cfg(unix)]
    #[test]
    fn a_finished_job_leaves_nothing_registered() {
        let token = CancelToken::new();
        let child = sleeping_child();
        token.attach(Arc::clone(&child));

        token.finish(&child);

        assert!(!token.is_attached());
        assert!(!token.is_cancelled());
        stop(&child);
    }

    #[cfg(unix)]
    #[test]
    fn a_cancel_after_the_finish_kills_nothing_and_leaves_the_next_process_alone() {
        let token = CancelToken::new();
        let first = sleeping_child();
        token.attach(Arc::clone(&first));
        token.finish(&first);
        stop(&first);

        token.cancel();

        let second = sleeping_child();
        assert!(
            second
                .wait_timeout(Duration::from_millis(50))
                .expect("the second process is ours to poll")
                .is_none(),
            "a cancel reached a process that was never registered"
        );
        stop(&second);
    }

    #[cfg(unix)]
    #[test]
    fn a_waiter_learns_that_a_process_was_registered_without_sleeping() {
        let token = Arc::new(CancelToken::new());
        let waiting = Arc::clone(&token);
        let waiter = std::thread::spawn(move || waiting.wait_attached(ENDED));

        let child = sleeping_child();
        token.attach(Arc::clone(&child));

        assert!(
            waiter.join().expect("the waiter finishes"),
            "no registration"
        );
        stop(&child);
    }
}
