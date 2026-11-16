// SPDX-License-Identifier: MIT
//! Chain of paste methods: the first available one, and on a refusal the next one
//!
//! The point of the chain is that the failure of one method must not cost the user a phrase. So
//! `ClipboardOnlyInjector` closes the chain: it always works, and the user finishes with Ctrl+V.
//! Each attempt goes to `InjectReport.attempts`, so `doctor` shows what exactly did not work,
//! and not just "it failed"

use std::sync::Arc;
use std::time::Duration;

use tracing::warn;

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

/// Paste method inside the chain. It is a concrete enum, not a `Box<dyn TextInjector>`, because
/// the paste key combination must change between phrases: a terminal needs Ctrl+Shift+V
#[derive(Debug)]
enum Backend {
    Hyprctl(HyprctlInjector),
    Wtype(WtypeInjector),
    Ydotool(YdotoolInjector),
    /// Boxed: the enigo connection is an order of magnitude larger than the other variants
    Enigo(Box<EnigoInjector>),
    #[cfg(target_os = "linux")]
    Uinput(UinputInjector),
    /// Any implementation: for tests and for tracks that add their own method
    Other(Box<dyn TextInjector>),
}

impl Backend {
    /// Name the two output settings take for this method
    ///
    /// A match with no wildcard, so a new variant has to decide the name a setting may use
    fn name(&self) -> &'static str {
        match self {
            Backend::Hyprctl(_) => "hyprctl",
            Backend::Wtype(_) => "wtype",
            Backend::Ydotool(_) => "ydotool",
            Backend::Enigo(_) => "enigo",
            #[cfg(target_os = "linux")]
            Backend::Uinput(_) => "uinput",
            Backend::Other(_) => "other",
        }
    }

    /// Whether this method delivers the mode
    ///
    /// `HyprctlInjector::inject` refuses every mode but a paste: it sends the shortcut and types
    /// no text, so it is the one method whose ability separates the two output settings
    fn can(&self, mode: OutputMode) -> bool {
        match self {
            Backend::Hyprctl(_) => mode == OutputMode::Paste,
            _ => true,
        }
    }

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

/// The methods `output.paste_backend` and `output.type_backend` accept
///
/// Every name here comes from `Backend::name`, and
/// `every_name_a_setting_may_take_is_a_method_the_chain_builds` fails when the two disagree.
/// `Other` is absent: it is a seam for tests and extra tracks, not a method a user can pick
pub const BACKEND_NAMES: &[&str] = &["hyprctl", "wtype", "ydotool", "enigo", "uinput"];

/// Move the method a setting names in front of the platform order
///
/// A preference, not a choice: the rest of the chain stays, because the named method may be
/// absent on this platform or may refuse the phrase, and a lost preference must not cost the user
/// the text. Every reason to ignore the value is a warning, because the platform order still
/// delivers
fn prefer(
    backends: &mut Vec<Backend>,
    setting: &str,
    key: &str,
    mode: OutputMode,
    platform: Platform,
) {
    if setting.eq_ignore_ascii_case("auto") {
        return;
    }
    if !BACKEND_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(setting))
    {
        warn!(
            "{key} = {setting:?}: no such paste method, expected `auto` or one of {}; \
             the automatic order stays",
            BACKEND_NAMES.join(", ")
        );
        return;
    }
    let Some(index) = backends
        .iter()
        .position(|backend| backend.name().eq_ignore_ascii_case(setting))
    else {
        warn!(
            "{key} = {setting:?}: no such method on {}, the automatic order stays",
            platform.label()
        );
        return;
    };
    if !backends[index].can(mode) {
        let what = if mode == OutputMode::Type {
            "type text"
        } else {
            "paste"
        };
        warn!("{key} = {setting:?}: this method cannot {what}, the automatic order stays");
        return;
    }
    let chosen = backends.remove(index);
    backends.insert(0, chosen);
}

/// The last method: put the text in the clipboard and tell the user to press Ctrl+V
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
            &format!("the text is in the clipboard, press {shortcut}"),
        );
        Ok(InjectReport {
            method: "clipboard-only".into(),
            attempts: Vec::new(),
        })
    }
}

/// Try the paste methods in an order that depends on the platform
#[derive(Debug)]
pub struct ChainInjector {
    backends: Vec<Backend>,
    fallback: ClipboardOnlyInjector,
    /// Look at the class of the active window and switch to Ctrl+Shift+V in terminals
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

    /// Chain for the current platform, in the order from the plan: first what needs no permissions
    ///
    /// `output.type_backend` and `output.paste_backend` move the method the user names to the
    /// front of that order. The paste setting is applied last, so it holds the front when both
    /// name different methods: a phrase that cannot be typed falls back to a paste through this
    /// same chain, and that makes the paste preference the wider one
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
                // wtype goes first: of the three it is the only one that can both type and paste,
                // and it is tested on Hyprland 0.56. hyprctl needs no permissions and no
                // protocols, but its sendshortcut does not always reach the client — so it is the
                // fallback, not the first
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
                // KDE and GNOME have no virtual-keyboard protocol, so wtype is useless
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

        prefer(
            &mut backends,
            &output.type_backend,
            "output.type_backend",
            OutputMode::Type,
            *platform,
        );
        prefer(
            &mut backends,
            &output.paste_backend,
            "output.paste_backend",
            OutputMode::Paste,
            *platform,
        );

        Self {
            backends,
            fallback: ClipboardOnlyInjector::system(notifier),
            terminal_shortcut: output.terminal_shortcut,
        }
    }

    /// Adjust the paste key combination to the active window
    pub fn apply_window(&mut self, class: Option<&str>) {
        if !self.terminal_shortcut {
            return;
        }
        let terminal = class.is_some_and(is_terminal_class);
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

    /// One pass through the chain. `unsupported_chars` is set if a method refused because of
    /// characters that are not in the keyboard layout
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
                attempts.push(format!("{id}: unavailable"));
                continue;
            }
            match injector.inject(text, mode) {
                Ok(report) => return Some(report),
                Err(InjectError::UnsupportedCharacters) => {
                    *unsupported_chars = true;
                    attempts.push(format!("{id}: the layout does not cover the text"));
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

    /// The first available method that can do it takes the selection; the other errors are
    /// collected in one message, so the user sees why it failed
    fn copy_selection(&mut self) -> Result<String, InjectError> {
        let mut reasons = Vec::new();
        for backend in &mut self.backends {
            let injector = backend.as_injector();
            let id = injector.id();
            if !injector.available() {
                continue;
            }
            match injector.copy_selection() {
                Ok(selection) => return Ok(selection),
                Err(InjectError::Unsupported) => {}
                Err(err) => reasons.push(format!("{id}: {err}")),
            }
        }
        if reasons.is_empty() {
            Err(InjectError::Unsupported)
        } else {
            Err(InjectError::Failed(reasons.join("; ")))
        }
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        // `Auto` must not reach this point, but if it does, resolve it by the general rule
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
            // Cyrillic cannot be typed blindly, so this phrase goes through paste instead
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

/// Clipboard under guard: a convenient constructor for tracks that need only the clipboard
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

    /// The test needs to see what went into the clipboard, so the content lives behind an `Arc`
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
        /// The method can paste but cannot type: this is how hyprctl and uinput behave
        /// on Cyrillic
        refuse_type: bool,
        /// What `copy_selection` returns; `None` is the default "this method cannot copy"
        selection: Option<String>,
        seen: Arc<Mutex<Vec<(String, OutputMode)>>>,
    }

    impl FakeInjector {
        /// Common base: the other constructors differ in exactly one field
        fn new(id: &'static str, seen: Arc<Mutex<Vec<(String, OutputMode)>>>) -> Self {
            Self {
                id,
                available: true,
                error: None,
                refuse_type: false,
                selection: None,
                seen,
            }
        }

        fn working(
            id: &'static str,
            seen: Arc<Mutex<Vec<(String, OutputMode)>>>,
        ) -> Box<dyn TextInjector> {
            Box::new(Self::new(id, seen))
        }

        fn failing(id: &'static str, error: InjectError) -> Box<dyn TextInjector> {
            Box::new(Self {
                error: Some(error),
                ..Self::new(id, Arc::new(Mutex::new(Vec::new())))
            })
        }

        fn missing(id: &'static str) -> Box<dyn TextInjector> {
            Box::new(Self {
                available: false,
                ..Self::new(id, Arc::new(Mutex::new(Vec::new())))
            })
        }

        /// A method that pastes but does not type
        fn paste_only(
            id: &'static str,
            seen: Arc<Mutex<Vec<(String, OutputMode)>>>,
        ) -> Box<dyn TextInjector> {
            Box::new(Self {
                refuse_type: true,
                ..Self::new(id, seen)
            })
        }

        /// A method that can copy the current selection
        fn copying(id: &'static str, selection: &str) -> Box<dyn TextInjector> {
            Box::new(Self {
                selection: Some(selection.to_string()),
                ..Self::new(id, Arc::new(Mutex::new(Vec::new())))
            })
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

        fn copy_selection(&mut self) -> Result<String, InjectError> {
            if let Some(err) = &self.error {
                return Err(err.clone());
            }
            self.selection.clone().ok_or(InjectError::Unsupported)
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
        assert_eq!(report.attempts, vec!["wtype: unavailable".to_string()]);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_failing_backend_hands_over_to_the_next_one() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("no window".into())),
            FakeInjector::working("wtype", seen.clone()),
        ]);
        let report = chain.inject("текст", OutputMode::Paste).unwrap();
        assert_eq!(report.method, "wtype");
        assert_eq!(report.attempts.len(), 1);
        assert!(
            report.attempts[0].contains("no window"),
            "{:?}",
            report.attempts
        );
    }

    #[test]
    fn when_everything_fails_the_text_lands_in_the_clipboard_with_a_notification() {
        let (mut chain, clipboard, notifier) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("no window".into())),
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
        assert!(seen.lock().unwrap().is_empty(), "no paste is needed");
        assert_eq!(*clipboard.0.text.lock().unwrap(), Some("реплика".into()));
    }

    #[test]
    fn unsupported_characters_in_type_mode_fall_back_to_paste() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::paste_only("uinput", seen.clone()),
            FakeInjector::paste_only("ydotool", seen.clone()),
        ]);
        // No method types Cyrillic blindly — the phrase goes out by paste
        let report = chain.inject("привет", OutputMode::Type).unwrap();
        assert_eq!(report.method, "uinput+fallback-paste");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("привет".to_string(), OutputMode::Paste)],
            "there was no typing, there was a paste"
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
    fn the_first_backend_that_can_copy_the_selection_wins() {
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::copying("hyprctl", "выделенный текст"),
            FakeInjector::copying("uinput", "другой текст"),
        ]);
        assert_eq!(chain.copy_selection().unwrap(), "выделенный текст");
    }

    #[test]
    fn a_refused_copy_hands_over_to_the_next_backend() {
        // The reason voice editing died: hyprctl was the only method that could copy, and its
        // refusal ended the whole chain
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("non-zero exit code".into())),
            FakeInjector::copying("uinput", "выделенный текст"),
        ]);
        assert_eq!(chain.copy_selection().unwrap(), "выделенный текст");
    }

    #[test]
    fn a_backend_without_copy_support_is_not_a_reason_for_the_error() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("exit 7".into())),
            FakeInjector::working("uinput", seen),
        ]);
        let error = chain.copy_selection().unwrap_err();
        assert_eq!(
            error,
            InjectError::Failed("hyprctl: paste failed: exit 7".into())
        );
    }

    #[test]
    fn every_refused_copy_is_reported_in_one_message() {
        let (mut chain, _clip, _n) = chain(vec![
            FakeInjector::failing("hyprctl", InjectError::Failed("exit 7".into())),
            FakeInjector::failing("uinput", InjectError::Failed("no /dev/uinput".into())),
        ]);
        let error = chain.copy_selection().unwrap_err().to_string();
        assert!(error.contains("hyprctl"), "{error}");
        assert!(error.contains("uinput"), "{error}");
    }

    #[test]
    fn a_chain_with_no_copy_method_is_unsupported() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (mut chain, _clip, _n) = chain(vec![FakeInjector::working("wtype", seen)]);
        assert_eq!(chain.copy_selection(), Err(InjectError::Unsupported));
    }

    #[test]
    fn the_terminal_shortcut_is_on_by_default() {
        // Criterion AM-08: paste into a terminal works out of the box, with no settings change
        let notifier = Arc::new(RecordingNotifier::default());
        let mut chain =
            ChainInjector::for_platform(&OutputConfig::default(), &Platform::X11, notifier);
        assert!(chain.terminal_shortcut, "on by default");
        chain.apply_window(Some("kitty"));
        assert!(chain.fallback.terminal, "a terminal needs Ctrl+Shift+V");
        chain.apply_window(Some("code"));
        assert!(!chain.fallback.terminal, "an editor needs plain Ctrl+V");
    }

    #[test]
    fn terminal_shortcut_is_off_unless_the_config_asks_for_it() {
        let (mut chain, _clip, _n) = chain(vec![]);
        chain.apply_window(Some("kitty"));
        assert!(!chain.fallback.terminal);
        chain.terminal_shortcut = true;
        chain.apply_window(Some("kitty"));
        assert!(chain.fallback.terminal, "a terminal needs Ctrl+Shift+V");
        chain.apply_window(Some("firefox"));
        assert!(!chain.fallback.terminal);
    }

    #[test]
    fn the_window_from_the_trait_reaches_the_chain() {
        // The pipeline calls `set_window` through the contract, not through a concrete type
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

    #[cfg(target_os = "linux")]
    #[test]
    fn the_hyprland_chain_keeps_a_copy_path_that_needs_no_compositor() {
        // Voice editing must not depend on the compositor alone: when `sendshortcut` is refused,
        // the virtual keyboard has to be able to press Ctrl+C itself
        let notifier = Arc::new(RecordingNotifier::default());
        let mut chain = ChainInjector::for_platform(
            &OutputConfig::default(),
            &Platform::Wayland(Compositor::Hyprland),
            notifier,
        );
        let ids: Vec<&str> = chain
            .backends
            .iter_mut()
            .map(|b| b.as_injector().id())
            .collect();
        assert!(ids.contains(&"uinput"), "{ids:?}");
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
            "KDE has no virtual-keyboard: {ids:?}"
        );
    }

    /// The methods of a chain built for `platform`, in the order it will try them
    fn chain_ids(platform: Platform, paste: &str, typing: &str) -> Vec<&'static str> {
        let output = OutputConfig {
            paste_backend: paste.into(),
            type_backend: typing.into(),
            ..OutputConfig::default()
        };
        let notifier = Arc::new(RecordingNotifier::default());
        let mut chain = ChainInjector::for_platform(&output, &platform, notifier);
        chain
            .backends
            .iter_mut()
            .map(|backend| backend.as_injector().id())
            .collect()
    }

    /// The chain without one method: what stays once a setting puts that method in front
    fn without<'a>(ids: &[&'a str], name: &str) -> Vec<&'a str> {
        ids.iter().copied().filter(|id| *id != name).collect()
    }

    /// The warnings a call writes out
    ///
    /// The subscriber belongs to the calling thread, so a test that runs beside this one neither
    /// reads these lines nor loses its own
    fn warnings_of(run: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if let Ok(mut sink) = self.0.lock() {
                    sink.extend_from_slice(buf);
                }
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let sink = Sink::default();
        let writer = sink.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, run);
        let bytes = sink.0.lock().map(|sink| sink.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    fn the_automatic_order_is_the_one_the_platform_gives() {
        // `auto` promises that an untouched settings file keeps the order the plan gives
        let hyprland = Platform::Wayland(Compositor::Hyprland);
        let ids = chain_ids(hyprland, "auto", "auto");
        #[cfg(target_os = "linux")]
        assert_eq!(ids, ["wtype", "hyprctl", "uinput", "ydotool"], "{ids:?}");
        #[cfg(not(target_os = "linux"))]
        assert_eq!(ids, ["wtype", "hyprctl", "ydotool"], "{ids:?}");
        assert_eq!(
            chain_ids(hyprland, "AUTO", "Auto"),
            ids,
            "the value is a name, not a spelling"
        );
        let x11 = chain_ids(Platform::X11, "auto", "auto");
        #[cfg(target_os = "linux")]
        assert_eq!(x11, ["enigo", "uinput"], "{x11:?}");
        #[cfg(not(target_os = "linux"))]
        assert_eq!(x11, ["enigo"], "{x11:?}");
    }

    #[test]
    fn a_named_paste_backend_leads_the_chain_and_the_rest_keeps_its_order() {
        let platform = Platform::Wayland(Compositor::Hyprland);
        let auto = chain_ids(platform, "auto", "auto");
        let ids = chain_ids(platform, "ydotool", "auto");
        assert_eq!(ids.first(), Some(&"ydotool"), "{ids:?}");
        assert_eq!(
            ids[1..],
            without(&auto, "ydotool")[..],
            "nothing is dropped and nothing else moves: {ids:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_named_type_backend_leads_the_chain_and_the_rest_keeps_its_order() {
        // uinput types ASCII on every Wayland session, so this preference is a real one
        let platform = Platform::Wayland(Compositor::Hyprland);
        let auto = chain_ids(platform, "auto", "auto");
        let ids = chain_ids(platform, "auto", "uinput");
        assert_eq!(ids.first(), Some(&"uinput"), "{ids:?}");
        assert_eq!(
            ids[1..],
            without(&auto, "uinput")[..],
            "nothing is dropped and nothing else moves: {ids:?}"
        );
    }

    #[test]
    fn an_unknown_backend_keeps_the_automatic_order_and_says_so() {
        let platform = Platform::X11;
        let mut order = Vec::new();
        let text = warnings_of(|| order = chain_ids(platform, "telepathy", "telepathy"));
        assert_eq!(order, chain_ids(platform, "auto", "auto"), "{order:?}");
        assert!(text.contains("output.paste_backend"), "{text}");
        assert!(text.contains("output.type_backend"), "{text}");
        assert!(text.contains("telepathy"), "{text}");
    }

    #[test]
    fn a_backend_the_platform_does_not_offer_is_a_lost_preference_only() {
        // X11 has no wtype: the setting must add nothing and take nothing away
        let platform = Platform::X11;
        let mut order = Vec::new();
        let text = warnings_of(|| order = chain_ids(platform, "wtype", "wtype"));
        assert_eq!(order, chain_ids(platform, "auto", "auto"), "{order:?}");
        assert!(!order.contains(&"wtype"), "{order:?}");
        assert!(text.contains("wtype"), "{text}");
        assert!(text.contains("X11"), "{text}");
    }

    #[test]
    fn hyprctl_leads_a_paste_but_never_a_typing_order() {
        // hyprctl synthesizes the paste shortcut and types nothing: it is the one method whose
        // ability separates the two settings
        let platform = Platform::Wayland(Compositor::Hyprland);
        assert_eq!(
            chain_ids(platform, "hyprctl", "auto").first(),
            Some(&"hyprctl")
        );
        let mut order = Vec::new();
        let text = warnings_of(|| order = chain_ids(platform, "auto", "hyprctl"));
        assert_eq!(order, chain_ids(platform, "auto", "auto"), "{order:?}");
        assert!(text.contains("output.type_backend"), "{text}");
    }

    #[test]
    fn with_both_settings_the_paste_preference_holds_the_front() {
        // A phrase that cannot be typed falls back to a paste through this same chain, so the
        // paste setting is the wider of the two and it wins the front
        let platform = Platform::Wayland(Compositor::Hyprland);
        let ids = chain_ids(platform, "hyprctl", "ydotool");
        assert_eq!(ids.first(), Some(&"hyprctl"), "{ids:?}");
        assert_eq!(ids.get(1), Some(&"ydotool"), "{ids:?}");
    }

    #[test]
    fn every_name_a_setting_may_take_is_a_method_the_chain_builds() {
        let mut built: Vec<&'static str> = Vec::new();
        for platform in [
            Platform::Wayland(Compositor::Hyprland),
            Platform::Wayland(Compositor::Sway),
            Platform::Wayland(Compositor::Kde),
            Platform::Wayland(Compositor::Gnome),
            Platform::Wayland(Compositor::Other),
            Platform::X11,
            Platform::Windows,
            Platform::MacOs,
            Platform::Headless,
        ] {
            let notifier = Arc::new(RecordingNotifier::default());
            let mut chain =
                ChainInjector::for_platform(&OutputConfig::default(), &platform, notifier);
            for backend in &mut chain.backends {
                let name = backend.name();
                if !built.contains(&name) {
                    built.push(name);
                }
            }
        }
        #[cfg(target_os = "linux")]
        for name in BACKEND_NAMES {
            assert!(
                built.contains(name),
                "{name} is no method of any platform: {built:?}"
            );
        }
        for name in &built {
            assert!(
                BACKEND_NAMES.contains(name),
                "{name} is a method no setting can name: {BACKEND_NAMES:?}"
            );
        }
    }
}
