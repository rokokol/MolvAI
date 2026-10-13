// SPDX-License-Identifier: MIT
//! State machine of the daemon: `Idle → Recording → Transcribing → PostProcessing → Injecting → Idle`
//!
//! It has no audio and no threads. The input is an event, and the output is a list of actions
//! and, maybe, an error for the reply to the client. The time comes from `Clock`, so tests
//! check all the rules about key hold without a single real pause
//!
//! The main input rule: **key hold and toggle are available at the same time**, and the
//! settings need no choice between them. A long press is push-to-talk: recording runs while the
//! key is held and ends at the release. A short tap of the same key (`hotkeys.tap_toggles`)
//! latches the recording in hands-free mode, and it continues until the next press. The
//! separate key `hotkeys.toggle` always starts and stops recording by a press, independent of
//! the first key

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::HotkeysConfig;
use crate::domain::clock::Clock;
use crate::domain::entry::Mode;
use crate::domain::hotkeys::{HotkeyAction, HotkeyEvent, KeyState};
use crate::ipc::protocol::{DaemonState, ErrorCode, IpcError};

/// Why a recording ended without producing a phrase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscardReason {
    /// The key was released before `min_hold_ms`: this is a mistake, not dictation
    TooShort,
    /// The user cancelled the recording explicitly
    Cancelled,
}

impl DiscardReason {
    /// Text for the notification; an empty string means "stay silent"
    pub fn message(self) -> &'static str {
        match self {
            DiscardReason::TooShort => "press too short, recording discarded",
            DiscardReason::Cancelled => "recording cancelled",
        }
    }
}

/// What the daemon must do after a transition
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Turn on capture from the microphone
    StartCapture { mode: Mode, style: Option<String> },
    /// Stop capture and send the audio to processing
    StopCaptureAndProcess { mode: Mode, style: Option<String> },
    /// Stop capture and throw away the recorded audio
    DiscardCapture { reason: DiscardReason },
}

/// Input of the machine: an IPC command, a hotkey event or a signal of the daemon itself
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
    /// `audio.max_duration_secs` is reached
    MaxDuration,
    /// Processing moved to the next step of the pipeline
    ProcessingStage(DaemonState),
    ProcessingDone,
    ProcessingFailed,
    Hotkey(HotkeyEvent),
}

/// Result of a transition: what to do and what to reply to the client
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub error: Option<IpcError>,
}

impl Outcome {
    /// The transition is accepted, and there is nothing to do
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
                hint: Some("wait for the processing to end or press the key again".into()),
            }),
        }
    }

    pub fn is_ok(&self) -> bool {
        self.error.is_none()
    }
}

/// A recording in progress
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub mode: Mode,
    pub style: Option<String>,
    pub since: Instant,
    /// The recording is "latched" by a short press and continues without the key held
    pub latched: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    Recording(Recording),
    /// Processing: `Transcribing`, `PostProcessing` or `Injecting`
    Processing(DaemonState),
}

/// State machine. One instance per daemon, and it lives in the control thread
#[derive(Debug)]
pub struct Machine {
    phase: Phase,
    hotkeys: HotkeysConfig,
    clock: Arc<dyn Clock>,
    /// Moment of the press that the machine accepted: the window for double-press suppression
    last_press: Option<Instant>,
    /// How many phrases are in processing now. The recording of the next phrase starts without
    /// waiting for the end of the previous processing: the microphone is free, and the jobs
    /// for the worker thread queue up by themselves. The machine goes to "ready" only when all
    /// are processed
    in_flight: u32,
}

impl Machine {
    pub fn new(hotkeys: HotkeysConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            phase: Phase::Idle,
            hotkeys,
            clock,
            last_press: None,
            in_flight: 0,
        }
    }

    /// How many phrases are still in processing
    pub fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// State visible to clients
    pub fn state(&self) -> DaemonState {
        match &self.phase {
            Phase::Idle => DaemonState::Idle,
            Phase::Recording(_) => DaemonState::Recording,
            Phase::Processing(stage) => *stage,
        }
    }

    /// Capture has really started: the hold counts from this moment, not from the start command.
    /// Opening the microphone takes up to several hundred milliseconds, and without this
    /// correction a short tap would look like a hold longer than `short_press_ms`, and the
    /// recording would end empty instead of latching
    pub fn capture_started(&mut self, at: Instant) {
        if let Phase::Recording(rec) = &mut self.phase {
            rec.since = at;
        }
    }

    /// The recording in progress, if there is one: `status` needs it for the duration
    pub fn recording(&self) -> Option<&Recording> {
        match &self.phase {
            Phase::Recording(rec) => Some(rec),
            _ => None,
        }
    }

    /// New hotkey configuration after `config.reload`
    pub fn set_hotkeys(&mut self, hotkeys: HotkeysConfig) {
        self.hotkeys = hotkeys;
    }

    /// Handle an input. For hotkey events the time comes from the event itself
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
            // The release of toggle/cancel and a style change do not move the state machine
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
                self.in_flight = self.in_flight.saturating_sub(1);
                // While the next recording runs, the machine stays in it; "ready" comes when
                // all phrases are processed, not the first of them
                if self.in_flight == 0 && matches!(self.phase, Phase::Processing(_)) {
                    self.phase = Phase::Idle;
                }
                Outcome::nothing()
            }
            Input::Hotkey(event) => self.on_hotkey(event),
        }
    }

    fn start(&mut self, mode: Mode, style: Option<String>, at: Instant) -> Outcome {
        match &self.phase {
            // While the previous phrase is in processing, the microphone is free: the next
            // recording starts at once, and its processing joins the queue of the worker thread
            Phase::Idle | Phase::Processing(_) => {
                self.last_press = Some(at);
                self.begin(mode, style, at, false)
            }
            Phase::Recording(rec) => {
                // The second press ends hands-free, but only outside the double-press window:
                // key bounce and auto-repeat must not cut a recording that has just started
                let since_press = self
                    .last_press
                    .map_or(Duration::MAX, |p| at.saturating_duration_since(p));
                if rec.latched && since_press >= Self::ms(self.hotkeys.double_tap_ms) {
                    self.last_press = Some(at);
                    return self.finish();
                }
                Outcome::busy("a recording is already running")
            }
        }
    }

    fn stop(&mut self, at: Instant) -> Outcome {
        let Phase::Recording(rec) = &mut self.phase else {
            // A stop outside a recording is no error: a key release after a cancel is normal
            return Outcome::nothing();
        };
        let held = at.saturating_duration_since(rec.since);
        if !rec.latched && self.hotkeys.tap_toggles && held < Self::ms(self.hotkeys.short_press_ms)
        {
            // A short tap turns on hands-free: the recording continues without the key held.
            // The hold does not go away: a long press of the same key stays push-to-talk, and
            // both ways work at the same time (AM-01)
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
            Phase::Idle | Phase::Processing(_) => {
                self.last_press = Some(at);
                // Toggle is latched at once: the key release does not stop the recording
                self.begin(mode, style, at, true)
            }
            Phase::Recording(_) => self.toggle_off(at),
        }
    }

    /// Toggle during a recording always ends it, however short the press is
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

    /// End the recording and go to processing
    fn finish(&mut self) -> Outcome {
        let Phase::Recording(rec) = &self.phase else {
            return Outcome::nothing();
        };
        let action = Action::StopCaptureAndProcess {
            mode: rec.mode,
            style: rec.style.clone(),
        };
        self.phase = Phase::Processing(DaemonState::Transcribing);
        self.in_flight += 1;
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
        assert!(
            out.actions.is_empty(),
            "the second recording must not start"
        );
        assert_eq!(out.error.unwrap().code, ErrorCode::Busy);
        assert_eq!(m.state(), DaemonState::Recording);
    }

    #[test]
    fn start_while_processing_begins_the_next_recording_at_once() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        assert_eq!(m.state(), DaemonState::Transcribing);
        assert_eq!(m.in_flight(), 1);

        // The microphone is free: the next phrase records while the first one is in processing
        clock.advance(Duration::from_secs(1));
        let out = m.on(start());
        assert!(out.is_ok(), "{out:?}");
        assert!(
            matches!(out.actions.as_slice(), [Action::StartCapture { .. }]),
            "{:?}",
            out.actions
        );
        assert_eq!(m.state(), DaemonState::Recording);
    }

    #[test]
    fn idle_comes_only_after_every_reply_is_processed() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_secs(1));
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        assert_eq!(m.in_flight(), 2);
        assert_eq!(m.state(), DaemonState::Transcribing);

        // The first phrase is processed and the second is not yet: it is too early for "ready"
        m.on(Input::ProcessingDone);
        assert_eq!(m.state(), DaemonState::Transcribing);
        assert_eq!(m.in_flight(), 1);
        m.on(Input::ProcessingDone);
        assert_eq!(m.state(), DaemonState::Idle);
        assert_eq!(m.in_flight(), 0);
    }

    #[test]
    fn a_reply_finishing_during_the_next_recording_does_not_stop_it() {
        let (mut m, clock) = machine();
        m.on(start());
        clock.advance(Duration::from_secs(2));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_secs(1));
        m.on(start());
        m.on(Input::ProcessingDone);
        assert_eq!(
            m.state(),
            DaemonState::Recording,
            "the end of processing of the first phrase does not touch the running recording"
        );
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
        assert_eq!(m.state(), DaemonState::Idle, "processing does not start");
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
        assert!(out.actions.is_empty(), "a tap must not stop the recording");
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
        // A tap latches the recording, and an instant second press is bounce
        clock.advance(Duration::from_millis(80));
        m.on(Input::RecordStop);
        clock.advance(Duration::from_millis(100));
        let second = m.on(start());
        assert!(
            second.actions.is_empty(),
            "bounce does not stop the recording"
        );
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
        // Toggle latches the recording at once: a short press does not cancel it and does not
        // turn it into a miss by `min_hold_ms`
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
        // A recording started by toggle must not die because the key was released fast:
        // this is exactly how hands-free differs from key hold
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
    fn hold_and_toggle_both_work() {
        // Criterion AM-01: key hold and toggle are available at the same time, on the same settings
        let (mut m, clock) = machine();

        // Key hold: the recording runs while the key is held and ends at the release
        m.on(start());
        clock.advance(Duration::from_millis(900));
        let held = m.on(Input::RecordStop);
        assert_eq!(
            held.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }],
            "key hold must work with tap_toggles = true"
        );
        m.on(Input::ProcessingDone);

        // Toggle: press to record, press again to finish, and no need to hold the key
        clock.advance(Duration::from_secs(1));
        let on = m.on(toggle());
        assert_eq!(
            on.actions,
            vec![Action::StartCapture {
                mode: Mode::Dictation,
                style: None
            }]
        );
        clock.advance(Duration::from_secs(2));
        let off = m.on(toggle());
        assert_eq!(
            off.actions,
            vec![Action::StopCaptureAndProcess {
                mode: Mode::Dictation,
                style: None
            }],
            "toggle must work on the same settings"
        );
        m.on(Input::ProcessingDone);

        // A short tap of the hold key also latches the recording: a third way on the same key
        clock.advance(Duration::from_secs(1));
        m.on(start());
        clock.advance(Duration::from_millis(100));
        let tapped = m.on(Input::RecordStop);
        assert!(tapped.actions.is_empty(), "{:?}", tapped.actions);
        assert!(m.recording().unwrap().latched);
    }

    #[test]
    fn slow_microphone_open_does_not_turn_a_tap_into_a_hold() {
        let (mut m, clock) = machine();
        m.on(start());
        // The microphone took 300 ms to open, which is longer than the short press threshold
        clock.advance(Duration::from_millis(300));
        m.capture_started(clock.instant());
        // The key was released 50 ms after the real start of capture: this is a tap
        clock.advance(Duration::from_millis(50));
        let out = m.on(Input::RecordStop);
        assert!(out.actions.is_empty(), "{:?}", out.actions);
        assert!(
            m.recording().unwrap().latched,
            "a tap after a slow microphone open must latch the recording"
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
        assert!(out.is_ok(), "a repeated stop is no error");
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
