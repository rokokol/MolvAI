// SPDX-License-Identifier: MIT

use std::ffi::OsStr;

pub(crate) const AUTOSTART_ARG: &str = "--autostart";

pub(crate) fn is_autostart(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
    args.into_iter()
        .skip(1)
        .any(|arg| arg.as_ref() == AUTOSTART_ARG)
}

pub(crate) fn initially_visible(autostart: bool, start_in_tray: bool) -> bool {
    !(autostart && start_in_tray)
}

#[derive(Default)]
pub(crate) struct Startup {
    setup_finished: bool,
    reveal_requested: bool,
    tray_ready: bool,
}

impl Startup {
    pub(crate) fn secondary_instance(&mut self, autostart: bool) -> bool {
        if autostart {
            return false;
        }
        self.reveal_requested = true;
        self.setup_finished
    }

    pub(crate) fn finish_setup(&mut self, tray_ready: bool) -> bool {
        self.setup_finished = true;
        self.tray_ready = tray_ready;
        !tray_ready || self.reveal_requested
    }

    pub(crate) fn hide_on_close(&self) -> bool {
        self.tray_ready
    }
}

pub(crate) enum TrayOutcome<E> {
    Ready,
    Error(E),
    Panicked,
}

pub(crate) fn try_tray<E>(init: impl FnOnce() -> Result<(), E>) -> TrayOutcome<E> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(init)) {
        Ok(Ok(())) => TrayOutcome::Ready,
        Ok(Err(err)) => TrayOutcome::Error(err),
        Err(_) => TrayOutcome::Panicked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_autostart_with_opt_in_starts_hidden() {
        for autostart in [false, true] {
            for start_in_tray in [false, true] {
                assert_eq!(
                    initially_visible(autostart, start_in_tray),
                    !(autostart && start_in_tray)
                );
            }
        }
    }

    #[test]
    fn autostart_requires_an_exact_argument_after_the_executable() {
        assert!(is_autostart(["molva-gui", "--autostart"]));
        assert!(is_autostart(["molva-gui", "--other", "--autostart"]));
        assert!(!is_autostart(["--autostart"]));
        assert!(!is_autostart(["molva-gui", "--autostart=false"]));
        assert!(!is_autostart(["molva-gui", "prefix--autostart"]));
        assert!(!is_autostart(["molva-gui"]));
    }

    #[test]
    fn secondary_autostart_never_requests_a_window() {
        let mut startup = Startup::default();
        assert!(!startup.secondary_instance(true));
        assert!(!startup.finish_setup(true));
        assert!(startup.hide_on_close());
        assert!(!startup.secondary_instance(true));
    }

    #[test]
    fn early_manual_request_survives_a_later_autostart_callback() {
        let mut startup = Startup::default();
        assert!(!startup.secondary_instance(false));
        assert!(!startup.secondary_instance(true));
        assert!(startup.finish_setup(true));
        assert!(startup.secondary_instance(false));
        assert!(!startup.secondary_instance(true));
    }

    #[test]
    fn tray_failure_reveals_the_window_and_allows_normal_close() {
        let mut startup = Startup::default();
        assert!(!startup.hide_on_close());
        assert!(startup.finish_setup(false));
        assert!(!startup.hide_on_close());
        assert!(startup.secondary_instance(false));
    }

    #[test]
    fn tray_errors_and_panics_both_fail_closed_to_the_tray() {
        assert!(matches!(
            try_tray(|| Ok::<(), &str>(())),
            TrayOutcome::Ready
        ));
        assert!(matches!(
            try_tray(|| Err("unavailable")),
            TrayOutcome::Error("unavailable")
        ));
        assert!(matches!(
            try_tray(|| -> Result<(), &str> { panic!("tray panic") }),
            TrayOutcome::Panicked
        ));
    }
}
