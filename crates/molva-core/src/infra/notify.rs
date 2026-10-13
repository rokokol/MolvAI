// SPDX-License-Identifier: MIT
//! Desktop notifications
//!
//! A notification always says what to do next ("press Ctrl+V"), and does not only report a
//! failure: at that moment the user has already lost sight of the phrase

use crate::domain::notify::Notifier;

/// Notifications through the standard desktop service
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemNotifier;

impl SystemNotifier {
    pub fn new() -> Self {
        Self
    }
}

impl Notifier for SystemNotifier {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn notify(&self, title: &str, body: &str) {
        // A missing notification service must not crash the dictation: write to the log and go on
        if let Err(error) = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .appname("MolvAI")
            .show()
        {
            tracing::debug!(%error, title, body, "notification not shown");
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "notification");
    }
}

/// Notifications to the log: for `--foreground` and for environments with no notification service
#[derive(Debug, Default, Clone, Copy)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "notification");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_notifier_never_fails_and_is_shareable() {
        let notifier: std::sync::Arc<dyn Notifier> = std::sync::Arc::new(LogNotifier);
        notifier.notify("MolvAI", "the text is in the clipboard");
    }
}
