// SPDX-License-Identifier: MIT
//! A job that a client may give up on, and the child process it is waiting for
//!
//! Waiting for a flag between two steps is not enough for a file of hours: the model works on
//! the whole of it in one call, and the wait would last as long as the recognition. The token
//! carries the process identifier of the recognizer, so a cancel reaches the process itself:
//! the model dies, the call returns at once, and nothing of the job reaches the journal

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Shared flag and process of one cancellable job
#[derive(Debug, Default)]
pub struct CancelToken {
    cancelled: AtomicBool,
    /// Process identifier of the child that does the work; zero means no child
    child: AtomicU32,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the client has given up
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// Give up: the flag goes up first, so a step that starts after this call sees it, and then
    /// the child is killed
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let pid = self.child.load(Ordering::Relaxed);
        if pid != 0 {
            kill_process(pid);
        }
    }

    /// Take the process that does the work, so a cancel can reach it
    pub fn attach(&self, pid: u32) {
        self.child.store(pid, Ordering::Relaxed);
    }

    /// The child is gone: a cancel after this point has nothing to kill
    pub fn detach(&self) {
        self.child.store(0, Ordering::Relaxed);
    }
}

/// Kill one process by its identifier
///
/// A process that has already exited makes this a no-op: the identifier may already belong to
/// somebody else, and a kill then fails with `ESRCH` before it does any harm
#[allow(unsafe_code)]
fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: `kill` with a signal number and a process identifier has no preconditions the
        // caller can break; a wrong identifier fails with an error instead of misbehaving
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        // No portable way without a handle held by the waiting thread: the flag still stops the
        // job between its steps
        let _ = pid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_token_is_not_cancelled() {
        assert!(!CancelToken::new().is_cancelled());
    }

    #[test]
    fn a_cancel_is_visible_after_the_call() {
        let token = CancelToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn an_attached_process_that_does_not_exist_is_harmless() {
        let token = CancelToken::new();
        token.attach(4_000_000);
        token.cancel();
        assert!(token.is_cancelled());
    }
}
