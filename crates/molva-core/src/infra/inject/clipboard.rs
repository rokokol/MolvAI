// SPDX-License-Identifier: MIT
//! Clipboard: a snapshot of the old content, a swap for the time of the paste, and the restore
//!
//! The user copies a link, dictates a phrase — and expects the link to stay in the clipboard.
//! So a paste through the clipboard always runs under `ClipboardGuard`, and is never "put it
//! there and forget". An empty clipboard is restored empty, not with the text of the previous
//! phrase

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::domain::inject::InjectError;

/// Snapshot of the clipboard content, enough for the restore
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClipboardSnapshot {
    #[default]
    Empty,
    Text(String),
    /// Uncompressed RGBA — what the system clipboards work with
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
}

/// Access to the system clipboard. The trait lets tests check `ClipboardGuard`
/// with no graphical session
pub trait ClipboardBackend: std::fmt::Debug + Send {
    fn snapshot(&mut self) -> ClipboardSnapshot;
    fn set_text(&mut self, text: &str) -> Result<(), InjectError>;
    fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<(), InjectError>;
}

/// Swaps the clipboard for the time of the paste and restores the old content
#[derive(Debug)]
pub struct ClipboardGuard<B: ClipboardBackend> {
    backend: B,
    restore_enabled: bool,
    restore_delay: Duration,
    saved: Option<ClipboardSnapshot>,
}

impl<B: ClipboardBackend> ClipboardGuard<B> {
    pub fn new(backend: B, restore_enabled: bool, restore_delay: Duration) -> Self {
        Self {
            backend,
            restore_enabled,
            restore_delay,
            saved: None,
        }
    }

    /// Put the text in the clipboard and remember what was there
    pub fn stage(&mut self, text: &str) -> Result<(), InjectError> {
        if self.restore_enabled && self.saved.is_none() {
            self.saved = Some(self.backend.snapshot());
        }
        self.backend.set_text(text)
    }

    /// Restore the old content. The pause lets the application finish reading the clipboard
    /// before the swap
    pub fn restore(&mut self) -> Result<(), InjectError> {
        let Some(saved) = self.saved.take() else {
            return Ok(());
        };
        if !self.restore_delay.is_zero() {
            std::thread::sleep(self.restore_delay);
        }
        self.backend.restore(&saved)
    }

    /// Cancel the restore: the text must stay in the clipboard (the paste failed)
    pub fn keep(&mut self) {
        self.saved = None;
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

/// System clipboard: arboard, and if it did not start, `wl-copy`/`wl-paste`
///
/// On Wayland arboard needs the `wlr-data-control` protocol; on compositors without it
/// (and in sessions without permission for the protocol) the external utilities remain
///
/// There is a second, less obvious reason to keep `wl-copy`: on Wayland the clipboard content
/// lives in the owner process. A short-lived command such as `molva test-inject` puts the text
/// through arboard, exits — and the clipboard becomes empty. `wl-copy` stays resident and goes
/// on to serve it, so the write always goes through it when it exists
#[derive(Default)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
    external_write: bool,
}

// `arboard::Clipboard` is a foreign type with no `Debug`, so we print not the connection but
// whether it exists
impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemClipboard")
            .field("arboard", &self.inner.is_some())
            .field("external_write", &self.external_write)
            .finish()
    }
}

impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            inner: arboard::Clipboard::new().ok(),
            external_write: which::which("wl-copy").is_ok(),
        }
    }

    /// Clipboard through arboard only: for environments without `wl-copy` and for tests
    pub fn arboard_only() -> Self {
        Self {
            inner: arboard::Clipboard::new().ok(),
            external_write: false,
        }
    }

    /// Whether any way to work with the clipboard is available
    pub fn available() -> bool {
        arboard::Clipboard::new().is_ok() || which::which("wl-copy").is_ok()
    }
}

impl ClipboardBackend for SystemClipboard {
    fn snapshot(&mut self) -> ClipboardSnapshot {
        if let Some(clipboard) = self.inner.as_mut() {
            if let Ok(text) = clipboard.get_text() {
                if !text.is_empty() {
                    return ClipboardSnapshot::Text(text);
                }
            }
            if let Ok(image) = clipboard.get_image() {
                return ClipboardSnapshot::Image {
                    width: image.width,
                    height: image.height,
                    rgba: image.bytes.into_owned(),
                };
            }
        }
        // arboard may not have seen the content (no protocol, a foreign owner) — we ask the
        // utility, or "empty" would turn the restore of the clipboard into a clear
        match wl_paste() {
            Some(text) if !text.is_empty() => ClipboardSnapshot::Text(text),
            _ => ClipboardSnapshot::Empty,
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), InjectError> {
        if self.external_write && wl_copy(text).is_ok() {
            return Ok(());
        }
        if let Some(clipboard) = self.inner.as_mut() {
            if clipboard.set_text(text).is_ok() {
                return Ok(());
            }
        }
        wl_copy(text)
    }

    fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<(), InjectError> {
        match snapshot {
            ClipboardSnapshot::Text(text) => self.set_text(text),
            ClipboardSnapshot::Empty => {
                if let Some(clipboard) = self.inner.as_mut() {
                    if clipboard.clear().is_ok() {
                        return Ok(());
                    }
                }
                wl_clear()
            }
            ClipboardSnapshot::Image {
                width,
                height,
                rgba,
            } => {
                let Some(clipboard) = self.inner.as_mut() else {
                    // The external utilities cannot restore an image: it is more honest to clear
                    // the clipboard than to leave a foreign phrase in it
                    return wl_clear();
                };
                clipboard
                    .set_image(arboard::ImageData {
                        width: *width,
                        height: *height,
                        bytes: std::borrow::Cow::Borrowed(rgba),
                    })
                    .map_err(|e| InjectError::ClipboardDenied(e.to_string()))
            }
        }
    }
}

/// Text from the clipboard through `wl-paste`; `None` if the utility is missing or the clipboard
/// holds no text. Both the clipboard snapshot and command mode need it: command mode takes the
/// selection after Ctrl+C
pub(crate) fn wl_paste() -> Option<String> {
    let output = Command::new("wl-paste").arg("--no-newline").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn wl_copy(text: &str) -> Result<(), InjectError> {
    // wl-copy goes to the background by itself and stays the owner of the selection; it returns
    // only after the clipboard is captured, so waiting for its exit is not only possible but
    // needed: otherwise Ctrl+V goes out before the text appears in the clipboard
    let status = Command::new("wl-copy")
        .arg("--")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| InjectError::ClipboardDenied(format!("wl-copy did not start: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(InjectError::ClipboardDenied(
            "wl-copy exited with an error".into(),
        ))
    }
}

/// Clear the clipboard through `wl-copy --clear`. Command mode clears it before Ctrl+C, to
/// tell a fresh selection from what was in the clipboard before
pub(crate) fn wl_clear() -> Result<(), InjectError> {
    let status = Command::new("wl-copy")
        .arg("--clear")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| InjectError::ClipboardDenied(format!("wl-copy --clear: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(InjectError::ClipboardDenied(
            "could not clear the clipboard".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clipboard in memory: the test sees each swap and restore
    #[derive(Debug, Default)]
    struct FakeClipboard {
        content: ClipboardSnapshot,
        writes: Vec<String>,
        fail_set: bool,
    }

    impl ClipboardBackend for FakeClipboard {
        fn snapshot(&mut self) -> ClipboardSnapshot {
            self.content.clone()
        }
        fn set_text(&mut self, text: &str) -> Result<(), InjectError> {
            if self.fail_set {
                return Err(InjectError::ClipboardDenied("no access".into()));
            }
            self.writes.push(text.to_string());
            self.content = ClipboardSnapshot::Text(text.to_string());
            Ok(())
        }
        fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<(), InjectError> {
            self.content = snapshot.clone();
            Ok(())
        }
    }

    fn guard(initial: ClipboardSnapshot) -> ClipboardGuard<FakeClipboard> {
        let backend = FakeClipboard {
            content: initial,
            ..FakeClipboard::default()
        };
        ClipboardGuard::new(backend, true, Duration::ZERO)
    }

    #[test]
    fn previous_text_comes_back_after_the_paste() {
        let mut guard = guard(ClipboardSnapshot::Text("ссылка".into()));
        guard.stage("новая реплика").unwrap();
        assert_eq!(
            guard.backend_mut().content,
            ClipboardSnapshot::Text("новая реплика".into())
        );
        guard.restore().unwrap();
        assert_eq!(
            guard.backend_mut().content,
            ClipboardSnapshot::Text("ссылка".into())
        );
        assert_eq!(
            guard.backend_mut().writes,
            vec!["новая реплика".to_string()]
        );
    }

    #[test]
    fn empty_clipboard_stays_empty() {
        let mut guard = guard(ClipboardSnapshot::Empty);
        guard.stage("реплика").unwrap();
        guard.restore().unwrap();
        assert_eq!(guard.backend_mut().content, ClipboardSnapshot::Empty);
    }

    #[test]
    fn image_in_the_clipboard_survives_the_paste() {
        let image = ClipboardSnapshot::Image {
            width: 2,
            height: 1,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let mut guard = guard(image.clone());
        guard.stage("реплика").unwrap();
        guard.restore().unwrap();
        assert_eq!(guard.backend_mut().content, image);
    }

    #[test]
    fn restore_disabled_leaves_the_reply_in_the_clipboard() {
        let backend = FakeClipboard {
            content: ClipboardSnapshot::Text("ссылка".into()),
            ..FakeClipboard::default()
        };
        let mut guard = ClipboardGuard::new(backend, false, Duration::ZERO);
        guard.stage("реплика").unwrap();
        guard.restore().unwrap();
        assert_eq!(
            guard.backend_mut().content,
            ClipboardSnapshot::Text("реплика".into())
        );
    }

    #[test]
    fn keep_cancels_the_restore_so_a_failed_paste_leaves_the_text() {
        let mut guard = guard(ClipboardSnapshot::Text("ссылка".into()));
        guard.stage("реплика").unwrap();
        guard.keep();
        guard.restore().unwrap();
        assert_eq!(
            guard.backend_mut().content,
            ClipboardSnapshot::Text("реплика".into())
        );
    }

    #[test]
    fn two_stages_in_a_row_remember_the_original_content_only_once() {
        let mut guard = guard(ClipboardSnapshot::Text("ссылка".into()));
        guard.stage("первая").unwrap();
        guard.stage("вторая").unwrap();
        guard.restore().unwrap();
        assert_eq!(
            guard.backend_mut().content,
            ClipboardSnapshot::Text("ссылка".into())
        );
    }

    #[test]
    fn clipboard_denial_is_reported_not_swallowed() {
        let backend = FakeClipboard {
            fail_set: true,
            ..FakeClipboard::default()
        };
        let mut guard = ClipboardGuard::new(backend, true, Duration::ZERO);
        let error = guard.stage("реплика").unwrap_err();
        assert!(matches!(error, InjectError::ClipboardDenied(_)), "{error}");
    }
}
