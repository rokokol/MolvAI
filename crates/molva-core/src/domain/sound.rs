// SPDX-License-Identifier: MIT
//! Sound cues of the recording: a short signal at the start and at the end of a phrase
//!
//! Criterion: the user hears a **start-of-recording signal and an end-of-recording signal** —
//! exactly two sounds for one phrase, and no extra one. Sound is the only feedback when the
//! MolvAI window is not visible, so it is tied to the transitions of the state machine, not to
//! input events
//!
//! Second criterion: **the signals can be turned off by a setting**, `audio.sounds = false`. The
//! switch-off lives in the implementation, not in the calling code: the daemon always calls
//! [`SoundCue::play`], and `NullSoundCue` from `infra::sound` makes the silence. So no branch can
//! forget to check "turned off"

/// Which signal to play
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueKind {
    /// Recording started: the microphone is open, you can speak
    RecordStart,
    /// Recording ended: the microphone is closed, the phrase went to processing
    RecordStop,
    /// Something failed: the microphone did not open, the recording was dropped
    Error,
}

impl CueKind {
    /// Tone frequency in hertz: the start is higher than the end, the error is much lower than both
    pub fn frequency_hz(self) -> f32 {
        match self {
            CueKind::RecordStart => 880.0,
            CueKind::RecordStop => 660.0,
            CueKind::Error => 220.0,
        }
    }

    /// Signal duration in milliseconds: shorter than a mouse click, so it does not disturb speech
    pub fn duration_ms(self) -> u32 {
        match self {
            CueKind::RecordStart | CueKind::RecordStop => 80,
            CueKind::Error => 160,
        }
    }

    /// Name for the logs
    pub fn label(self) -> &'static str {
        match self {
            CueKind::RecordStart => "recording start",
            CueKind::RecordStop => "recording end",
            CueKind::Error => "error",
        }
    }
}

/// Player of sound cues
///
/// An implementation must return at once: the daemon calls `play` from the control thread,
/// and a wait for the sound would delay the opening of the microphone
pub trait SoundCue: std::fmt::Debug + Send + Sync {
    /// Name of the implementation: it shows whether anything sounds at all (`null` — turned off by a setting)
    fn id(&self) -> &'static str;
    fn play(&self, kind: CueKind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_stop_sound_different() {
        assert_ne!(
            CueKind::RecordStart.frequency_hz(),
            CueKind::RecordStop.frequency_hz(),
            "the start and the end of the recording must sound different"
        );
        assert!(CueKind::Error.frequency_hz() < CueKind::RecordStop.frequency_hz());
    }

    #[test]
    fn a_cue_is_short_enough_not_to_interrupt_speech() {
        for kind in [CueKind::RecordStart, CueKind::RecordStop, CueKind::Error] {
            assert!(kind.duration_ms() <= 200, "{kind:?}");
        }
    }
}
