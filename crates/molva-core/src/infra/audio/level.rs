// SPDX-License-Identifier: MIT
//! Watch of the signal level during a recording
//!
//! The most annoying loss of a phrase is a switched-off or muted microphone: the recording runs,
//! the indicator is on, and the buffer holds a digital zero. This module notices it within a
//! second and reports it once per silence, so notifications do not turn into noise

use std::time::{Duration, Instant};

/// Below this RMS there is effectively no signal: −80 dBFS is the silence of a digital zero, not quiet speech
pub const ZERO_LEVEL_RMS: f32 = 1e-4;

/// How long to wait before a zero level counts as a problem and not as a pause before a word
pub const ZERO_LEVEL_WINDOW: Duration = Duration::from_secs(1);

/// A break in the measurements after which a new recording starts. Inside a recording the
/// measurements come at the `LEVEL_INTERVAL` step from `cpal_source`, and between recordings
/// there are none: the silence at the end of the previous recording must not count for the next one
pub const RECORDING_GAP: Duration = Duration::from_secs(2);

/// What to show to the user at a zero level
pub const ZERO_LEVEL_MESSAGE: &str =
    "The microphone gives no signal. Check that it is not switched off and that the right input \
     is selected (`molva devices`)";

/// Watches the level and says when it is time to warn about a zero input
///
/// The time comes from outside: the test does not sleep, and the daemon takes it from its clock
#[derive(Debug)]
pub struct ZeroLevelWatch {
    threshold: f32,
    window: Duration,
    /// When the current stretch of silence began
    silent_since: Option<Instant>,
    /// The warning for this stretch is already given
    warned: bool,
    /// Time of the previous measurement: a break longer than `RECORDING_GAP` is the border between recordings
    last_seen: Option<Instant>,
}

impl ZeroLevelWatch {
    pub fn new(threshold: f32, window: Duration) -> Self {
        Self {
            threshold,
            window,
            silent_since: None,
            warned: false,
            last_seen: None,
        }
    }

    /// Default values: −80 dBFS for longer than a second
    pub fn with_defaults() -> Self {
        Self::new(ZERO_LEVEL_RMS, ZERO_LEVEL_WINDOW)
    }

    /// Take the next level measurement; return the warning text if it is time to show it
    ///
    /// The warning is given once per continuous stretch of silence: when a signal appears, the
    /// counter resets, and the next loss is noticed again
    pub fn observe(&mut self, rms: f32, now: Instant) -> Option<&'static str> {
        if self
            .last_seen
            .is_some_and(|last| now.duration_since(last) >= RECORDING_GAP)
        {
            self.reset();
        }
        self.last_seen = Some(now);
        if rms > self.threshold {
            self.silent_since = None;
            self.warned = false;
            return None;
        }
        let since = *self.silent_since.get_or_insert(now);
        if self.warned || now.duration_since(since) < self.window {
            return None;
        }
        self.warned = true;
        Some(ZERO_LEVEL_MESSAGE)
    }

    /// Forget what was accumulated: a new recording starts from a clean slate
    pub fn reset(&mut self) {
        self.silent_since = None;
        self.warned = false;
    }
}

impl Default for ZeroLevelWatch {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_silence_is_not_reported() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        assert_eq!(watch.observe(0.0, t0), None);
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(900)),
            None,
            "a pause before the first word is no reason to alarm the user"
        );
    }

    #[test]
    fn silence_longer_than_the_window_is_reported_once() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);

        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(1100)),
            Some(ZERO_LEVEL_MESSAGE)
        );
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(1200)),
            None,
            "repeated warnings about the same silence"
        );
    }

    #[test]
    fn a_pause_between_recordings_does_not_carry_silence_into_the_next_one() {
        // The daemon watches the levels of all recordings with one watcher: there are no
        // measurements between recordings, and the silence at the end of the previous recording
        // must not count for the start of the next one
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.observe(0.0, t0 + Duration::from_millis(500));

        let next = t0 + Duration::from_secs(10);
        assert_eq!(watch.observe(0.0, next), None);
        assert_eq!(
            watch.observe(0.0, next + Duration::from_millis(500)),
            None,
            "half a second of silence in a new recording is not yet a reason to warn"
        );
        assert_eq!(
            watch.observe(0.0, next + Duration::from_millis(1100)),
            Some(ZERO_LEVEL_MESSAGE)
        );
    }

    #[test]
    fn signal_resets_the_watch() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.observe(0.0, t0 + Duration::from_millis(1100));

        watch.observe(0.2, t0 + Duration::from_millis(1200));

        assert_eq!(watch.observe(0.0, t0 + Duration::from_millis(1300)), None);
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(2400)),
            Some(ZERO_LEVEL_MESSAGE),
            "a signal lost a second time must be noticed again"
        );
    }

    #[test]
    fn quiet_speech_is_not_zero_level() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        // −60 dBFS: quiet, but this is a signal, not a switched-off microphone
        watch.observe(0.001, t0);
        assert_eq!(watch.observe(0.001, t0 + Duration::from_secs(5)), None);
    }

    #[test]
    fn reset_forgets_the_silent_stretch() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.reset();
        assert_eq!(watch.observe(0.0, t0 + Duration::from_millis(1100)), None);
    }
}
