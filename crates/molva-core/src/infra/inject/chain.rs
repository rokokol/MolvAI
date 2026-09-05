// SPDX-License-Identifier: MIT
//! Цепочка способов вставки: первый доступный, при отказе — следующий.
//!
//! Смысл цепочки в том, что отказ одного способа не должен стоить пользователю реплики. Поэтому
//! замыкает её `ClipboardOnlyInjector`: он работает всегда, а пользователь дожимает Ctrl+V сам.
//! Каждая попытка попадает в `InjectReport.attempts`, чтобы `doctor` показывал, что именно не
//! сработало, а не «не получилось».

use std::sync::Arc;
use std::time::Duration;

use crate::config::OutputConfig;
use crate::domain::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use crate::domain::notify::Notifier;
use crate::infra::inject::clipboard::{ClipboardBackend, ClipboardGuard, SystemClipboard};
use crate::infra::inject::wayland_tools::{
    HyprctlInjector, PasteShortcut, WtypeInjector, YdotoolInjector,
};
use crate::infra::inject::{enigo_inj::EnigoInjector, is_terminal_class};
use crate::infra::platform::{Compositor, Platform};

#[cfg(target_os = "linux")]
use crate::infra::inject::uinput::UinputInjector;

/// Способ вставки внутри цепочки. Конкретный enum, а не `Box<dyn TextInjector>`, потому что
/// сочетание вставки приходится менять между репликами: терминалу нужен Ctrl+Shift+V.
#[derive(Debug)]
enum Backend {
    Hyprctl(HyprctlInjector),
    Wtype(WtypeInjector),
    Ydotool(YdotoolInjector),
    /// В коробке: соединение enigo на порядок толще остальных вариантов.
    Enigo(Box<EnigoInjector>),
    #[cfg(target_os = "linux")]
    Uinput(UinputInjector),
    /// Произвольная реализация: для тестов и для дорожек, которые добавят свой способ.
    Other(Box<dyn TextInjector>),
}

impl Backend {
    fn set_shortcut(&mut self, shortcut: PasteShortcut) {
        match self {
            Backend::Hyprctl(i) => i.set_shortcut(shortcut),
            Backend::Wtype(i) => i.set_shortcut(shortcut),
            Backend::Ydotool(i) => i.set_shortcut(shortcut),
            Backend::Enigo(i) => i.set_shortcut(shortcut),
            #[cfg(target_os = "linux")]
            Backend::Uinput(i) => i.set_shortcut(shortcut),
            Backend::Other(_) => {}
        }
    }

    fn as_injector(&mut self) -> &mut dyn TextInjector {
        match self {
            Backend::Hyprctl(i) => i,
            Backend::Wtype(i) => i,
            Backend::Ydotool(i) => i,
            Backend::Enigo(i) => i.as_mut(),
            #[cfg(target_os = "linux")]
            Backend::Uinput(i) => i,
            Backend::Other(i) => i.as_mut(),
        }
    }
}

/// Последний способ: положить текст в буфер и сказать пользователю нажать Ctrl+V.
#[derive(Debug)]
pub struct ClipboardOnlyInjector {
    backend: Box<dyn ClipboardBackend>,
    notifier: Arc<dyn Notifier>,
    terminal: bool,
}

impl ClipboardOnlyInjector {
    pub fn new(backend: Box<dyn ClipboardBackend>, notifier: Arc<dyn Notifier>) -> Self {
        Self {
            backend,
            notifier,
            terminal: false,
        }
    }

    pub fn system(notifier: Arc<dyn Notifier>) -> Self {
        Self::new(Box::new(SystemClipboard::new()), notifier)
    }

    pub fn set_terminal(&mut self, terminal: bool) {
        self.terminal = terminal;
    }
}

impl TextInjector for ClipboardOnlyInjector {
    fn id(&self) -> &'static str {
        "clipboard-only"
    }

    fn available(&self) -> bool {
        true
    }

    fn inject(&mut self, text: &str, _mode: OutputMode) -> Result<InjectReport, InjectError> {
        self.backend.set_text(text)?;
        let shortcut = if self.terminal {
            "Ctrl+Shift+V"
        } else {
            "Ctrl+V"
        };
        self.notifier.notify(
            "MolvAI",
            &format!("текст в буфере обмена, нажмите {shortcut}"),
        );
        Ok(InjectReport {
            method: "clipboard-only".into(),
            attempts: Vec::new(),
        })
    }
}

/// Перебор способов вставки в порядке, который зависит от платформы.
#[derive(Debug)]
pub struct ChainInjector {
    backends: Vec<Backend>,
    fallback: ClipboardOnlyInjector,
    /// Смотреть на класс активного окна и переключаться на Ctrl+Shift+V в терминалах.
    terminal_shortcut: bool,
}

impl ChainInjector {
    pub fn new(
        backends: Vec<Box<dyn TextInjector>>,
        fallback: ClipboardOnlyInjector,
        terminal_shortcut: bool,
    ) -> Self {
        Self {
            backends: backends.into_iter().map(Backend::Other).collect(),
            fallback,
            terminal_shortcut,
        }
    }

    /// Цепочка для текущей платформы, в порядке из плана: сначала то, что не требует прав.
    pub fn for_platform(
        output: &OutputConfig,
        platform: &Platform,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        let restore = output.restore_clipboard;
        let delay = output.restore_delay_ms;
        let type_delay = output.type_delay_ms;
        let shortcut = PasteShortcut::CtrlV;
        let mut backends: Vec<Backend> = Vec::new();

        let wtype = || Backend::Wtype(WtypeInjector::new(restore, delay, shortcut, type_delay));
        let ydotool = || Backend::Ydotool(YdotoolInjector::new(restore, delay, shortcut));
        let enigo = || Backend::Enigo(Box::new(EnigoInjector::new(restore, delay, shortcut)));
        #[cfg(target_os = "linux")]
        let uinput = || Backend::Uinput(UinputInjector::new(restore, delay, shortcut, type_delay));

        match platform {
            Platform::Wayland(Compositor::Hyprland) => {
                // wtype первым: он единственный из троих умеет и набор, и вставку, и проверен
                // на Hyprland 0.56. hyprctl не требует ни прав, ни протоколов, но его
                // sendshortcut доходит до клиента не всегда — поэтому он запасной, а не первый.
                backends.push(wtype());
                backends.push(Backend::Hyprctl(HyprctlInjector::new(
                    restore, delay, shortcut,
                )));
                #[cfg(target_os = "linux")]
                backends.push(uinput());
                backends.push(ydotool());
            }
            Platform::Wayland(Compositor::Sway | Compositor::Other) => {
                backends.push(wtype());
                #[cfg(target_os = "linux")]
                backends.push(uinput());
                backends.push(ydotool());
            }
            Platform::Wayland(Compositor::Kde | Compositor::Gnome) => {
                // В KDE и GNOME протокола virtual-keyboard нет, wtype бесполезен.
                #[cfg(target_os = "linux")]
                backends.push(uinput());
                backends.push(ydotool());
            }
            Platform::X11 => {
                backends.push(enigo());
                #[cfg(target_os = "linux")]
                backends.push(uinput());
            }
            Platform::Windows | Platform::MacOs => backends.push(enigo()),
            Platform::Headless => {}
        }

        Self {
            backends,
            fallback: ClipboardOnlyInjector::system(notifier),
            terminal_shortcut: output.terminal_shortcut,
        }
    }

    /// Подстроить сочетание вставки под активное окно.
    pub fn apply_window(&mut self, class: Option<&str>) {
        if !self.terminal_shortcut {
            return;
        }
        let terminal = class.map(is_terminal_class).unwrap_or(false);
        let shortcut = if terminal {
            PasteShortcut::CtrlShiftV
        } else {
            PasteShortcut::CtrlV
        };
        for backend in &mut self.backends {
            backend.set_shortcut(shortcut);
        }
        self.fallback.set_terminal(terminal);
    }

    /// Один проход по цепочке. `unsupported_chars` поднимается, если способ отказался из-за
    /// символов, которых нет в раскладке.
    fn try_chain(
        &mut self,
        text: &str,
        mode: OutputMode,
        attempts: &mut Vec<String>,
        unsupported_chars: &mut bool,
    ) -> Option<InjectReport> {
        for backend in &mut self.backends {
            let injector = backend.as_injector();
            let id = injector.id();
            if !injector.available() {
                attempts.push(format!("{id}: недоступен"));
                continue;
            }
            match injector.inject(text, mode) {
                Ok(report) => return Some(report),
                Err(InjectError::UnsupportedCharacters) => {
                    *unsupported_chars = true;
                    attempts.push(format!("{id}: раскладка не покрывает текст"));
                }
                Err(err) => attempts.push(format!("{id}: {err}")),
            }
        }
        None
    }
}

impl TextInjector for ChainInjector {
    fn id(&self) -> &'static str {
        "chain"
    }

    fn available(&self) -> bool {
        true
    }

    fn set_window(&mut self, class: Option<&str>) {
        self.apply_window(class);
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        // `Auto` сюда доходить не должен, но если дошёл — разрешаем по общему правилу.
        let mode = mode.resolve(text, usize::MAX);
        if mode == OutputMode::Clipboard {
            return self.fallback.inject(text, mode);
        }

        let mut attempts = Vec::new();
        let mut unsupported_chars = false;
        if let Some(mut report) = self.try_chain(text, mode, &mut attempts, &mut unsupported_chars)
        {
            report.attempts = attempts;
            return Ok(report);
        }

        if mode == OutputMode::Type && unsupported_chars {
            // Кириллицу вслепую не набрать — для этой реплики переходим на вставку.
            let mut ignored = false;
            if let Some(mut report) =
                self.try_chain(text, OutputMode::Paste, &mut attempts, &mut ignored)
            {
                report.method = format!("{}+fallback-paste", report.method);
                report.attempts = attempts;
                return Ok(report);
            }
        }

        let mut report = self.fallback.inject(text, OutputMode::Clipboard)?;
        report.attempts = attempts;
        Ok(report)
    }
}

/// Буфер обмена под охраной: удобный конструктор для дорожек, которым нужен только он.
pub fn system_clipboard_guard(output: &OutputConfig) -> ClipboardGuard<SystemClipboard> {
    ClipboardGuard::new(
        SystemClipboard::new(),
        output.restore_clipboard,
        Duration::from_millis(u64::from(output.restore_delay_ms)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::RecordingNotifier;
    use std::sync::Mutex;

    #[derive(Debug, Default)]
    struct FakeClipboard {
        text: Mutex<Option<String>>,
    }

    /// Тесту нужно видеть, что попало в буфер, поэтому содержимое живёт за `Arc`.
    #[derive(Debug, Clone, Default)]
    struct SharedClipboard(Arc<FakeClipboard>);

    impl ClipboardBackend for SharedClipboard {
        fn snapshot(&mut self) -> crate::infra::inject::clipboard::ClipboardSnapshot {
            crate::infra::inject::clipboard::ClipboardSnapshot::Empty
        }
        fn set_text(&mut self, text: &str) -> Result<(), InjectError> {
            *self.0.text.lock().unwrap() = Some(text.to_string());
            Ok(())
        }
        fn restore(
            &mut self,
            _snapshot: &crate::infra::inject::clipboard::ClipboardSnapshot,
        ) -> Result<(), InjectError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeInjector {
        id: &'static str,
        available: bool,
        error: Option<InjectError>,
        /// Способ умеет вставлять, но не умеет набирать: так ведут себя hyprctl и uinput
        /// на кириллице.
        refuse_type: bool,
        seen: Arc<Mutex<Vec<(String, OutputMode)>>>,
    }

    impl FakeInjector {
        fn working(id: &'static str, seen: Arc<Mutex<Vec<(String, OutputMode)>>>) -> Box<Self> {
            Box::new(Self {
                id,
                available: true,
                error: None,
                refuse_type: false,
                seen,
            })
        }
        fn failing(id: &'static str, error: InjectError) -> Box<Self> {
            Box::new(Self {
                id,
                available: true,
                error: Some(error),
                refuse_type: false,
                seen: Arc::new(Mutex::new(Vec::new())),
            })
        }
        fn missing(id: &'static str) -> Box<Self> {
            Box::new(Self {
                id,
                available: false,
                error: None,
                refuse_type: false,
                seen: Arc::new(Mutex::new(Vec::new())),
            })
        }
        /// Способ, который вставляет, но не набирает.
        fn paste_only(id: &'static str, seen: Arc<Mutex<Vec<(String, OutputMode)>>>) -> Box<Self> {
            let mut injector = Self::working(id, seen);
            injector.refuse_type = true;
            injector
        }
    }

    impl TextInjector for FakeInjector {
        fn id(&self) -> &'static str {
            self.id
        }
        fn available(&self) -> bool {
            self.available
        }
        fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
            if let Some(err) = &self.error {
                return Err(err.clone());
            }
            if self.refuse_type && mode == OutputMode::Type {
                return Err(InjectError::UnsupportedCharacters);
            }
            self.seen.lock().unwrap().push((text.to_string(), mode));
            Ok(InjectReport {
                method: self.id.to_string(),
                attempts: Vec::new(),
            })
        }
    }

    fn chain(
        backends: Vec<Box<dyn TextInjector>>,
    ) -> (ChainInjector, SharedClipboard, Arc<RecordingNotifier>) {
        let clipboard = SharedClipboard::default();
        let notifier = Arc::new(RecordingNotifier::default());
        let fallback = ClipboardOnlyInjector::new(Box::new(clipboard.clone()), notifier.clone());
        (
            ChainInjector::new(backends, fallback, false),
            clipboard,
            notifier,
        )
    }

    #[test]
    fn the_first_available_backend_wins() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::working("first", seen.clone()),
            FakeInjector::working("second", Arc::new(Mutex::new(Vec::new()))),
        ]);
        let report = chain.inject("текст", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "first");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("текст".to_string(), OutputMode::Paste)]
        );
    }

    #[test]
    fn an_unavailable_backend_is_skipped_and_recorded() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::missing("wtype"),
            FakeInjector::working("uinput", seen.clone()),
        ]);
        let report = chain.inject("текст", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "uinput");
        assert_eq!(report.attempts, vec!["wtype: недоступен".to_string()]);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_failing_backend_hands_over_to_the_next_one() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("нет окна".into())),
            FakeInjector::working("wtype", seen.clone()),
        ]);
        let report = chain.inject("текст", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "wtype");
        assert_eq!(report.attempts.len(), 1);
        assert!(
            report.attempts[0].contains("нет окна"),
            "{:?}",
            report.attempts
        );
    }

    #[test]
    fn when_everything_fails_the_text_lands_in_the_clipboard_with_a_notification() {
        let (mut chain, clipboard, notifier) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("нет окна".into())),
            FakeInjector::missing("wtype"),
        ]);
        let report = chain.inject("реплика", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "clipboard-only");
        assert_eq!(report.attempts.len(), 2);
        assert_eq!(*clipboard.0.text.lock().unwrap(), Some("реплика".into()));
        let messages = notifier.messages.lock().unwrap();
        assert!(messages[0].1.contains("Ctrl+V"), "{:?}", messages[0]);
    }

    #[test]
    fn clipboard_mode_goes_straight_to_the_clipboard() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, clipboard, _n) = chain(vec![FakeInjector::working("wtype", seen.clone())]);
        let report = chain.inject("реплика", OutputMode::Clipboard).unwrap();
        assert_eq!(report.method, "clipboard-only");
        assert!(seen.lock().unwrap().is_empty(), "вставка не нужна");
        assert_eq!(*clipboard.0.text.lock().unwrap(), Some("реплика".into()));
    }

    #[test]
    fn unsupported_characters_in_type_mode_fall_back_to_paste() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::paste_only("uinput", seen.clone()),
            FakeInjector::paste_only("ydotool", seen.clone()),
        ]);
        // Ни один способ не набирает кириллицу вслепую — реплика уходит вставкой.
        let report = chain.inject("привет", OutputMode::Type).unwrap();
        assert_eq!(report.method, "uinput+fallback-paste");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("привет".to_string(), OutputMode::Paste)],
            "набора не было, была вставка"
        );
        assert_eq!(report.attempts.len(), 2, "{:?}", report.attempts);
    }

    #[test]
    fn a_typable_text_is_not_downgraded_to_paste() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![FakeInjector::working("wtype", seen.clone())]);
        let report = chain.inject("hello", OutputMode::Type).unwrap();
        assert_eq!(report.method, "wtype");
        assert_eq!(seen.lock().unwrap()[0].1, OutputMode::Type);
    }

    #[test]
    fn auto_is_resolved_before_the_chain_sees_it() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![FakeInjector::working("wtype", seen.clone())]);
        chain.inject("короткий текст", OutputMode::Auto).unwrap();
        assert_eq!(seen.lock().unwrap()[0].1, OutputMode::Type);
    }

    #[test]
    fn an_empty_chain_still_delivers_through_the_clipboard() {
        let (mut chain, clipboard, _n) = chain(vec![]);
        let report = chain.inject("реплика", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "clipboard-only");
        assert_eq!(*clipboard.0.text.lock().unwrap(), Some("реплика".into()));
    }

    #[test]
    fn terminal_shortcut_is_off_unless_the_config_asks_for_it() {
        let (mut chain, _clip, _n) = chain(vec![]);
        chain.apply_window(Some("kitty"));
        assert!(!chain.fallback.terminal);
        chain.terminal_shortcut = true;
        chain.apply_window(Some("kitty"));
        assert!(chain.fallback.terminal, "в терминале нужен Ctrl+Shift+V");
        chain.apply_window(Some("firefox"));
        assert!(!chain.fallback.terminal);
    }

    #[test]
    fn the_window_from_the_trait_reaches_the_chain() {
        // Конвейер зовёт `set_window` через контракт, а не через конкретный тип.
        let (mut chain, _clip, _n) = chain(vec![]);
        chain.terminal_shortcut = true;
        let injector: &mut dyn TextInjector = &mut chain;
        injector.set_window(Some("foot"));
        assert!(chain.fallback.terminal);
    }

    #[test]
    fn hyprland_chain_tries_wtype_first_and_keeps_hyprctl_as_a_spare() {
        let notifier = Arc::new(RecordingNotifier::default());
        let chain = ChainInjector::for_platform(
            &OutputConfig::default(),
            &Platform::Wayland(Compositor::Hyprland),
            notifier,
        );
        let mut backends = chain.backends;
        let ids: Vec<&str> = backends.iter_mut().map(|b| b.as_injector().id()).collect();
        assert_eq!(ids.first(), Some(&"wtype"), "{ids:?}");
        assert!(ids.contains(&"hyprctl"), "{ids:?}");
        assert!(
            ids.iter().position(|id| *id == "wtype") < ids.iter().position(|id| *id == "hyprctl"),
            "{ids:?}"
        );
    }

    #[test]
    fn kde_chain_does_not_offer_wtype() {
        let notifier = Arc::new(RecordingNotifier::default());
        let chain = ChainInjector::for_platform(
            &OutputConfig::default(),
            &Platform::Wayland(Compositor::Kde),
            notifier,
        );
        let mut backends = chain.backends;
        let ids: Vec<&str> = backends.iter_mut().map(|b| b.as_injector().id()).collect();
        assert!(
            !ids.contains(&"wtype"),
            "в KDE нет virtual-keyboard: {ids:?}"
        );
    }
}
