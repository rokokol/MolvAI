// SPDX-License-Identifier: MIT
//! Машина состояний демона: `Idle → Recording → Transcribing → PostProcessing → Injecting → Idle`.
//!
//! Здесь нет ни аудио, ни потоков: вход — событие, выход — список действий и, возможно, ошибка
//! для ответа клиенту. Время берётся из `Clock`, поэтому все правила про удержание клавиши
//! проверяются тестами без единой реальной паузы.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::HotkeysConfig;
use crate::domain::clock::Clock;
use crate::domain::entry::Mode;
use crate::domain::hotkeys::{HotkeyAction, HotkeyEvent, KeyState};
use crate::ipc::protocol::{DaemonState, ErrorCode, IpcError};

/// Почему запись закончилась, не породив реплику.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// Клавишу отпустили раньше `min_hold_ms`: это промах, а не диктовка.
    TooShort,
    /// Пользователь отменил запись явно.
    Cancelled,
}

impl DiscardReason {
    /// Текст для уведомления; пустая строка означает «молчать».
    pub fn message(self) -> &'static str {
        match self {
            DiscardReason::TooShort => "слишком короткое нажатие — запись отброшена",
            DiscardReason::Cancelled => "запись отменена",
        }
    }
}

/// Что демон обязан сделать после перехода.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Включить захват с микрофона.
    StartCapture { mode: Mode, style: Option<String> },
    /// Остановить захват и отправить аудио в обработку.
    StopCaptureAndProcess { mode: Mode, style: Option<String> },
    /// Остановить захват и выбросить записанное.
    DiscardCapture { reason: DiscardReason },
}

/// Вход машины: команда IPC, событие хоткея или сигнал самого демона.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    RecordStart {
        mode: Mode,
        style: Option<String>,
    },
    RecordStop,
    RecordToggle {
        mode: Mode,
        style: Option<String>,
    },
    RecordCancel,
    /// Достигнут `audio.max_duration_secs`.
    MaxDuration,
    /// Обработка перешла на следующий шаг конвейера.
    ProcessingStage(DaemonState),
    ProcessingDone,
    ProcessingFailed,
    Hotkey(HotkeyEvent),
}

/// Результат перехода: что делать и что ответить клиенту.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub error: Option<IpcError>,
}

impl Outcome {
    /// Переход принят, делать нечего.
    pub fn nothing() -> Self {
        Self::default()
    }

    fn act(action: Action) -> Self {
        Self {
            actions: vec![action],
            error: None,
        }
    }

    fn busy(message: &str) -> Self {
        Self {
            actions: Vec::new(),
            error: Some(IpcError {
                code: ErrorCode::Busy,
                message: message.to_string(),
                hint: Some("дождитесь окончания обработки или нажмите клавишу ещё раз".into()),
            }),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// Идущая запись.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub mode: Mode,
    pub style: Option<String>,
    pub since: Instant,
    /// Запись «защёлкнута» коротким нажатием и продолжается без удержания клавиши.
    pub latched: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    Recording(Recording),
    /// Обработка: `Transcribing`, `PostProcessing` или `Injecting`.
    Processing(DaemonState),
}

/// Машина состояний. Один экземпляр на демон, живёт в управляющем потоке.
#[derive(Debug)]
pub struct Machine {
    phase: Phase,
    hotkeys: HotkeysConfig,
    clock: Arc<dyn Clock>,
    /// Момент нажатия, которое машина приняла к исполнению: окно подавления двойного нажатия.
    last_press: Option<Instant>,
}

impl Machine {
    pub fn new(hotkeys: HotkeysConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            phase: Phase::Idle,
            hotkeys,
            clock,
            last_press: None,
        }
    }

    /// Состояние, видимое клиентам.
    pub fn state(&self) -> DaemonState {
        match &self.phase {
            Phase::Idle => DaemonState::Idle,
            Phase::Recording(_) => DaemonState::Recording,
            Phase::Processing(stage) => *stage,
        }
    }

    /// Идущая запись, если она есть: нужна `status` для длительности.
    pub fn recording(&self) -> Option<&Recording> {
        match &self.phase {
            Phase::Recording(rec) => Some(rec),
            _ => None,
        }
    }

    /// Новая конфигурация горячих клавиш после `config.reload`.
    pub fn set_hotkeys(&mut self, hotkeys: HotkeysConfig) {
        self.hotkeys = hotkeys;
    }

    /// Обработать вход. Для событий хоткеев время берётся из самого события.
    pub fn on(&mut self, input: Input) -> Outcome {
        match input {
            Input::Hotkey(event) => self.on_hotkey(event),
            other => {
                let at = self.clock.instant();
                self.apply(other, at)
            }
        }
    }

    fn on_hotkey(&mut self, event: HotkeyEvent) -> Outcome {
        let input = match (event.action, event.state) {
            (HotkeyAction::PushToTalk, KeyState::Pressed) => Some(Input::RecordStart {
                mode: Mode::Dictation,
                style: None,
            }),
            (HotkeyAction::Command, KeyState::Pressed) => Some(Input::RecordStart {
                mode: Mode::Command,
                style: None,
            }),
            (HotkeyAction::PushToTalk | HotkeyAction::Command, KeyState::Released) => {
                Some(Input::RecordStop)
            }
            (HotkeyAction::Toggle, KeyState::Pressed) => Some(Input::RecordToggle {
                mode: Mode::Dictation,
                style: None,
            }),
            (HotkeyAction::Cancel, KeyState::Pressed) => Some(Input::RecordCancel),
            // Отпускание toggle/cancel и смена стиля машину состояний не двигают.
            _ => None,
        };
        match input {
            Some(input) => self.apply(input, event.at),
            None => Outcome::nothing(),
        }
    }

    fn ms(value: u32) -> Duration {
        Duration::from_millis(u64::from(value))
    }

    fn apply(&mut self, input: Input, at: Instant) -> Outcome {
        match input {
            Input::RecordStart { mode, style } => self.start(mode, style, at),
            Input::RecordStop => self.stop(at),
            Input::RecordToggle { mode, style } => self.toggle(mode, style, at),
            Input::RecordCancel => self.cancel(),
            Input::MaxDuration => self.max_duration(),
            Input::ProcessingStage(stage) => {
                if let Phase::Processing(current) = &mut self.phase {
                    *current = stage;
                }
                Outcome::nothing()
            }
            Input::ProcessingDone | Input::ProcessingFailed => {
                if matches!(self.phase, Phase::Processing(_)) {
                    self.phase = Phase::Idle;
                }
                Outcome::nothing()
            }
            Input::Hotkey(event) => self.on_hotkey(event),
        }
    }

    fn start(&mut self, mode: Mode, style: Option<String>, at: Instant) -> Outcome {
        match &self.phase {
            Phase::Idle => {
                self.last_press = Some(at);
                self.begin(mode, style, at, false)
            }
            Phase::Recording(rec) => {
                // Второе нажатие завершает hands-free, но только вне окна двойного нажатия:
                // дребезг и автоповтор клавиши не должны обрывать только что начатую запись.
                let since_press = self
                    .last_press
                    .map_or(Duration::MAX, |p| at.saturating_duration_since(p));
                if rec.latched && since_press >= Self::ms(self.hotkeys.double_tap_ms) {
                    self.last_press = Some(at);
                    return self.finish();
                }
                Outcome::busy("запись уже идёт")
            }
            Phase::Processing(_) => Outcome::busy("идёт обработка предыдущей реплики"),
        }
    }

    fn stop(&mut self, at: Instant) -> Outcome {
        let Phase::Recording(rec) = &mut self.phase else {
            // Остановка вне записи — не ошибка: отпускание клавиши после отмены штатно.
            return Outcome::nothing();
        };
        let held = at.saturating_duration_since(rec.since);
        if !rec.latched && self.hotkeys.tap_toggles && held < Self::ms(self.hotkeys.short_press_ms)
        {
            // Короткий тап включает hands-free: запись продолжается без удержания клавиши.
            rec.latched = true;
            return Outcome::nothing();
        }
        if !rec.latched && held < Self::ms(self.hotkeys.min_hold_ms) {
            self.phase = Phase::Idle;
            return Outcome::act(Action::DiscardCapture {
                reason: DiscardReason::TooShort,
            });
        }
        self.finish()
    }

    fn toggle(&mut self, mode: Mode, style: Option<String>, at: Instant) -> Outcome {
        match &self.phase {
            Phase::Idle => {
                self.last_press = Some(at);
                // Toggle сразу защёлкнут: отпускание клавиши запись не остановит.
                self.begin(mode, style, at, true)
            }
            Phase::Recording(_) => self.toggle_off(at),
            Phase::Processing(_) => Outcome::busy("идёт обработка предыдущей реплики"),
        }
    }

    /// Toggle во время записи всегда завершает её, каким бы коротким ни было нажатие.
    fn toggle_off(&mut self, at: Instant) -> Outcome {
        self.last_press = Some(at);
        self.finish()
    }

    fn cancel(&mut self) -> Outcome {
        if matches!(self.phase, Phase::Recording(_)) {
            self.phase = Phase::Idle;
            return Outcome::act(Action::DiscardCapture {
                reason: DiscardReason::Cancelled,
            });
        }
        Outcome::nothing()
    }

    fn max_duration(&mut self) -> Outcome {
        if matches!(self.phase, Phase::Recording(_)) {
            return self.finish();
        }
        Outcome::nothing()
    }

    fn begin(&mut self, mode: Mode, style: Option<String>, at: Instant, latched: bool) -> Outcome {
        self.phase = Phase::Recording(Recording {
            mode,
            style: style.clone(),
            since: at,
            latched,
        });
        Outcome::act(Action::StartCapture { mode, style })
    }

    /// Закончить запись и уйти в обработку.
    fn finish(&mut self) -> Outcome {
        let Phase::Recording(rec) = &self.phase else {
            return Outcome::nothing();
        };
        let action = Action::StopCaptureAndProcess {
            mode: rec.mode,
            style: rec.style.clone(),
        };
        self.phase = Phase::Processing(DaemonState::Transcribing);
        Outcome::act(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::fakes::FakeClock;
    use chrono::{DateTime, Utc};

    fn clock() -> Arc<FakeClock> {
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        Arc::new(FakeClock::at(start))
    }

    fn machine_with(hotkeys: HotkeysConfig) -> (Machine, Arc<FakeClock>) {
        let clock = clock();
        (Machine::new(hotkeys, clock.clone()), clock)
    }

    fn machine() -> (Machine, Arc<FakeClock>) {
        machine_with(HotkeysConfig::default())
    }

    fn start() -> Input {
        Input::RecordStart {
            mode: Mode::Dictation,
            style: None,
        }
    }

    fn toggle() -> Input {
        Input::RecordToggle {
            mode: Mode::Dictation,
            style: None,
        }
    }

    #[test]
    fn start_from_idle_turns_on_capture() {
        let (mut m, _clock) = machine();
        let out = m.on(start());
        assert_eq!(
            out.actions,
            vec![Action::StartCapture {
                mode: Mode::Dictation,
                style: None
            }]
        );
        assert!(out.is_ok());
        assert_eq!(m.state(), DaemonState::Recording);
    }

    #[test]
    fn second_start_while_recording_is_busy_and_starts_nothing() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(1));
        let out = m.on(start());
        assert!(out.actions.is_empty(), "вторая запись не должна начаться");
        assert_eq!(out.error.unwrap().code, ErrorCode::Busy);
        assert_eq!(m.state(), DaemonState::Recording);
    }

    #[test]
    fn start_while_processing_is_busy() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        assert_eq!(m.state(), DaemonState::Transcribing);
        let out = m.on(start());
        assert!(out.actions.is_empty());
        assert_eq!(out.error.unwrap().code, ErrorCode::Busy);
    }

    #[test]
    fn hold_shorter_than_min_hold_discards_audio_without_a_reply() {
        let hotkeys = HotkeysConfig {
            tap_toggles: false,
            min_hold_ms: 200,
            ..HotkeysConfig::default()
        };
        let (mut m, clock) = machine_with(hotkeys);
        m.on(start());
        clock.advance(Duration::from_millis(120));
        let out = m.on(Input::RecordStop);
        assert_eq!(
            out.actions,
            vec![Action::DiscardCapture {
                reason: DiscardReason::TooShort
            }]
        );
        assert_eq!(m.state(), DaemonState::Idle, "обработка не запускается");
    }

    #[test]
    fn hold_longer_than_min_hold_goes_to_processing() {
        let hotkeys = HotkeysConfig {
            tap_toggles: false,
            ..HotkeysConfig::default()
        };
        let (mut m, clock) = machine_with(hotkeys);
        m.on(start());
        clock.advance(Duration::from_millis(900));
        let out = m.on(Input::RecordStop);
        assert_eq!(
            out.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
        assert_eq!(m.state(), DaemonState::Transcribing);
    }

    #[test]
    fn tap_latches_hands_free_and_recording_continues() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_millis(100));
        let out = m.on(Input::RecordStop);
        assert!(out.actions.is_empty(), "тап не должен останавливать запись");
        assert_eq!(m.state(), DaemonState::Recording);
        assert!(m.recording().unwrap().latched);
    }

    #[test]
    fn latched_recording_is_finished_by_the_next_press() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_millis(100));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_secs(3));
        let out = m.on(start());
        assert_eq!(
            out.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
        assert_eq!(m.state(), DaemonState::Transcribing);
    }

    #[test]
    fn explicit_stop_finishes_a_latched_recording() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_millis(100));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_secs(3));
        let out = m.on(Input::RecordStop);
        assert_eq!(
            out.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
    }

    #[test]
    fn double_press_inside_the_window_does_not_produce_two_recordings() {
        let (mut m, clock) = machine();
        let first = m.on(start());
        assert_eq!(first.actions.len(), 1);
        // Тап защёлкивает запись, а мгновенное второе нажатие — это дребезг.
        clock.advance(Duration::from_millis(80));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_millis(100));
        let second = m.on(start());
        assert!(second.actions.is_empty(), "дребезг не останавливает запись");
        assert_eq!(second.error.unwrap().code, ErrorCode::Busy);
        assert_eq!(m.state(), DaemonState::Recording);
    }

    #[test]
    fn toggle_from_idle_starts_and_toggle_while_recording_stops() {
        let (mut m, clock) = machine();
        let on = m.on(toggle());
        assert_eq!(
            on.actions,
            vec![Action::StartCapture {
                mode: Mode::Dictation,
                style: None
            }]
        );
        // Toggle защёлкивает запись сразу: короткое нажатие её не отменяет и не превращает
        // в промах по `min_hold_ms`.
        assert!(m.recording().unwrap().latched);
        clock.advance(Duration::from_secs(1));
        let off = m.on(toggle());
        assert_eq!(
            off.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
        assert_eq!(m.state(), DaemonState::Transcribing);
    }

    #[test]
    fn a_toggled_recording_is_short_press_proof() {
        // Запись, начатая toggle, не должна умереть от того, что клавишу отпустили быстро:
        // ровно этим отличается hands-free от удержания.
        let (mut m, clock) = machine();
        m.on(toggle());
        clock.advance(Duration::from_millis(30));
        let out = m.on(Input::RecordStop);
        assert!(
            !out.actions.contains(&Action::DiscardCapture {
                reason: DiscardReason::TooShort
            }),
            "{:?}",
            out.actions
        );
    }

    #[test]
    fn cancel_drops_the_recording_and_returns_to_idle() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        let out = m.on(Input::RecordCancel);
        assert_eq!(
            out.actions,
            vec![Action::DiscardCapture {
                reason: DiscardReason::Cancelled
            }]
        );
        assert_eq!(m.state(), DaemonState::Idle);
    }

    #[test]
    fn stop_during_processing_is_a_silent_no_op() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        let out = m.on(Input::RecordStop);
        assert!(out.actions.is_empty());
        assert!(out.is_ok(), "повторный stop не ошибка");
        assert_eq!(m.state(), DaemonState::Transcribing);
    }

    #[test]
    fn stop_in_idle_is_a_silent_no_op() {
        let (mut m, _clock) = machine();
        let out = m.on(Input::RecordStop);
        assert!(out.actions.is_empty());
        assert!(out.is_ok());
        assert_eq!(m.state(), DaemonState::Idle);
    }

    #[test]
    fn max_duration_finishes_the_recording() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(600));
        let out = m.on(Input::MaxDuration);
        assert_eq!(
            out.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
    }

    #[test]
    fn stages_advance_and_done_returns_to_idle() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        m.on(Input::ProcessingStage(DaemonState::PostProcessing));
        assert_eq!(m.state(), DaemonState::PostProcessing);
        m.on(Input::ProcessingStage(DaemonState::Injecting));
        assert_eq!(m.state(), DaemonState::Injecting);
        m.on(Input::ProcessingDone);
        assert_eq!(m.state(), DaemonState::Idle);
    }

    #[test]
    fn failed_processing_also_returns_to_idle() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        m.on(Input::ProcessingFailed);
        assert_eq!(m.state(), DaemonState::Idle);
    }

    #[test]
    fn push_to_talk_hotkey_runs_a_full_cycle() {
        let hotkeys = HotkeysConfig {
            tap_toggles: false,
            ..HotkeysConfig::default()
        };
        let (mut m, clock) = machine_with(hotkeys);
        let press = HotkeyEvent {
            action: HotkeyAction::PushToTalk,
            state: KeyState::Pressed,
            at: clock.instant(),
        };
        let on = m.on(Input::Hotkey(press));
        assert_eq!(
            on.actions,
            vec![Action::StartCapture {
                mode: Mode::Dictation,
                style: None
            }]
        );
        clock.advance(Duration::from_millis(1200));
        let release = HotkeyEvent {
            action: HotkeyAction::PushToTalk,
            state: KeyState::Released,
            at: clock.instant(),
        };
        let off = m.on(Input::Hotkey(release));
        assert_eq!(
            off.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }]
        );
    }

    #[test]
    fn command_hotkey_records_in_command_mode() {
        let hotkeys = HotkeysConfig {
            tap_toggles: false,
            ..HotkeysConfig::default()
        };
        let (mut m, clock) = machine_with(hotkeys);
        m.on(Input::Hotkey(HotkeyEvent {
            action: HotkeyAction::Command,
            state: KeyState::Pressed,
            at: clock.instant(),
        }));
        clock.advance(Duration::from_millis(800));
        let off = m.on(Input::Hotkey(HotkeyEvent {
            action: HotkeyAction::Command,
            state: KeyState::Released,
            at: clock.instant(),
        }));
        assert_eq!(
            off.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Command,
                style: None
            }]
        );
    }

    #[test]
    fn style_next_hotkey_does_not_move_the_machine() {
        let (mut m, clock) = machine();
        let out = m.on(Input::Hotkey(HotkeyEvent {
            action: HotkeyAction::StyleNext,
            state: KeyState::Pressed,
            at: clock.instant(),
        }));
        assert!(out.actions.is_empty());
        assert_eq!(m.state(), DaemonState::Idle);
    }

    #[test]
    fn cancel_hotkey_drops_the_recording() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(1));
        let out = m.on(Input::Hotkey(HotkeyEvent {
            action: HotkeyAction::Cancel,
            state: KeyState::Pressed,
            at: clock.instant(),
        }));
        assert_eq!(
            out.actions,
            vec![Action::DiscardCapture {
                reason: DiscardReason::Cancelled
            }]
        );
    }

    #[test]
    fn style_is_carried_from_start_to_processing() {
        let hotkeys = HotkeysConfig {
            tap_toggles: false,
            ..HotkeysConfig::default()
        };
        let (mut m, clock) = machine_with(hotkeys);
        m.on(Input::RecordStart {
            mode: Mode::Dictation,
            style: Some("formal".into()),
        });
        clock.advance(Duration::from_secs(1));
        let out = m.on(Input::RecordStop);
        assert_eq!(
            out.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: Some("formal".into())
            }]
        );
    }
}
