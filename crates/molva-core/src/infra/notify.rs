// SPDX-License-Identifier: MIT
//! Уведомления рабочего стола.
//!
//! Уведомление всегда говорит, что делать дальше («нажмите Ctrl+V»), а не просто сообщает о
//! сбое: пользователь в этот момент уже потерял свою реплику из виду.

use crate::domain::notify::Notifier;

/// Уведомления через штатную службу рабочего стола.
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
        // Отсутствие службы уведомлений не должно ронять диктовку: пишем в лог и живём дальше.
        if let Err(error) = notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .appname("MolvAI")
            .show()
        {
            tracing::debug!(%error, title, body, "уведомление не показано");
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "уведомление");
    }
}

/// Уведомления в лог: для `--foreground` и для окружений без службы уведомлений.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogNotifier;

impl Notifier for LogNotifier {
    fn notify(&self, title: &str, body: &str) {
        tracing::info!(title, body, "уведомление");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_notifier_never_fails_and_is_shareable() {
        let notifier: std::sync::Arc<dyn Notifier> = std::sync::Arc::new(LogNotifier);
        notifier.notify("MolvAI", "текст в буфере обмена");
    }
}
