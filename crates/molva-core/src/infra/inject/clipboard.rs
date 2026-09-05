// SPDX-License-Identifier: MIT
//! Буфер обмена: снимок прежнего содержимого, подмена на время вставки и возврат.
//!
//! Пользователь копирует ссылку, диктует реплику — и ждёт, что в буфере останется ссылка.
//! Поэтому вставка через буфер всегда идёт под `ClipboardGuard`, а не «положил и забыл».
//! Пустой буфер восстанавливается пустым, а не текстом предыдущей реплики.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::domain::inject::InjectError;

/// Снимок содержимого буфера, достаточный для возврата.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ClipboardSnapshot {
    #[default]
    Empty,
    Text(String),
    /// RGBA без сжатия — то, чем оперируют системные буферы.
    Image {
        width: usize,
        height: usize,
        rgba: Vec<u8>,
    },
}

/// Доступ к системному буферу обмена. Трейт нужен, чтобы `ClipboardGuard` проверялся тестами
/// без графической сессии.
pub trait ClipboardBackend: std::fmt::Debug + Send {
    fn snapshot(&mut self) -> ClipboardSnapshot;
    fn set_text(&mut self, text: &str) -> Result<(), InjectError>;
    fn restore(&mut self, snapshot: &ClipboardSnapshot) -> Result<(), InjectError>;
}

/// Подменяет буфер на время вставки и возвращает прежнее содержимое.
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

    /// Положить текст в буфер, запомнив то, что там было.
    pub fn stage(&mut self, text: &str) -> Result<(), InjectError> {
        if self.restore_enabled && self.saved.is_none() {
            self.saved = Some(self.backend.snapshot());
        }
        self.backend.set_text(text)
    }

    /// Вернуть прежнее содержимое. Пауза даёт приложению дочитать буфер до подмены.
    pub fn restore(&mut self) -> Result<(), InjectError> {
        let Some(saved) = self.saved.take() else {
            return Ok(());
        };
        if !self.restore_delay.is_zero() {
            std::thread::sleep(self.restore_delay);
        }
        self.backend.restore(&saved)
    }

    /// Отказаться от возврата: текст должен остаться в буфере (вставка не удалась).
    pub fn keep(&mut self) {
        self.saved = None;
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

/// Системный буфер: arboard, а если он не поднялся — `wl-copy`/`wl-paste`.
///
/// На Wayland arboard требует протокол `wlr-data-control`; на композиторах без него
/// (и в сессиях без прав на протокол) остаются внешние утилиты.
///
/// Есть вторая, менее очевидная причина держать `wl-copy`: на Wayland содержимое буфера живёт
/// в процессе-владельце. Короткоживущая команда вроде `molva test-inject` кладёт текст через
/// arboard, выходит — и буфер становится пустым. `wl-copy` остаётся резидентом и продолжает его
/// обслуживать, поэтому запись всегда идёт через него, когда он есть.
#[derive(Default)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
    external_write: bool,
}

// `arboard::Clipboard` — чужой тип без `Debug`, поэтому печатаем не соединение, а его наличие.
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

    /// Буфер только через arboard: для окружений без `wl-copy` и для тестов.
    pub fn arboard_only() -> Self {
        Self {
            inner: arboard::Clipboard::new().ok(),
            external_write: false,
        }
    }

    /// Доступен ли хоть какой-то способ работы с буфером.
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
        // arboard мог не увидеть содержимое (нет протокола, чужой владелец) — спросим утилиту,
        // иначе «пусто» превратит возврат буфера в его очистку.
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
                    // Картинку внешними утилитами не вернуть: честнее очистить буфер,
                    // чем оставить в нём чужую реплику.
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

fn wl_paste() -> Option<String> {
    let output = Command::new("wl-paste").arg("--no-newline").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn wl_copy(text: &str) -> Result<(), InjectError> {
    // wl-copy сам уходит в фон и остаётся владельцем выделения; управление он возвращает уже
    // после того, как буфер захвачен, поэтому ждать его выхода не только можно, но и нужно:
    // иначе Ctrl+V уйдёт раньше, чем в буфере появится текст.
    let status = Command::new("wl-copy")
        .arg("--")
        .arg(text)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| InjectError::ClipboardDenied(format!("wl-copy не запустился: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(InjectError::ClipboardDenied(
            "wl-copy завершился с ошибкой".into(),
        ))
    }
}

fn wl_clear() -> Result<(), InjectError> {
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
            "не удалось очистить буфер обмена".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Буфер в памяти: тест видит каждую подмену и возврат.
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
                return Err(InjectError::ClipboardDenied("нет доступа".into()));
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
        let err = guard.stage("реплика").unwrap_err();
        assert!(matches!(err, InjectError::ClipboardDenied(_)), "{err}");
    }
}
