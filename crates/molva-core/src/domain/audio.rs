// SPDX-License-Identifier: MIT
//! Audio: the PCM buffer, the capture source and the pure conversions
//!
//! The recognizer takes mono 16 kHz `f32`, so everything that comes from the microphone or from
//! a file is converted to this form here, whatever the backend

use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Sample rate that whisper expects
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Mono signal with a known sample rate
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PcmAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl PcmAudio {
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self {
            samples,
            sample_rate,
        }
    }

    /// Duration in seconds; 0 for an empty buffer or a zero sample rate
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// Root-mean-square level of the signal, 0.0 for an empty buffer
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.samples.iter().map(|s| s * s).sum();
        (sum / self.samples.len() as f32).sqrt()
    }

    /// Convert to 16 kHz; if the sample rate is already the target, a copy with no change
    #[must_use]
    pub fn to_16k(&self) -> PcmAudio {
        resample_linear(self, TARGET_SAMPLE_RATE)
    }
}

/// Mix interleaved channels down to mono by averaging
///
/// `channels == 0` counts as mono, so broken headers do not crash the pipeline
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().sum();
            sum / channels as f32
        })
        .collect()
}

/// Linear interpolation between neighbouring samples
///
/// This is enough for speech that goes to whisper. A high-quality polyphase resampler (rubato)
/// can be added later behind the same signature
pub fn resample_linear(audio: &PcmAudio, target_rate: u32) -> PcmAudio {
    if audio.sample_rate == target_rate || audio.samples.is_empty() || audio.sample_rate == 0 {
        return PcmAudio::new(audio.samples.clone(), target_rate);
    }
    let ratio = f64::from(audio.sample_rate) / f64::from(target_rate);
    let out_len = ((audio.samples.len() as f64) / ratio).round().max(1.0) as usize;
    let last = audio.samples.len() - 1;
    let samples = (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let index = (pos.floor() as usize).min(last);
            let next = (index + 1).min(last);
            let frac = (pos - index as f64) as f32;
            audio.samples[index] * (1.0 - frac) + audio.samples[next] * frac
        })
        .collect();
    PcmAudio::new(samples, target_rate)
}

/// Input device as the user sees it in the CLI and the GUI
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AudioError {
    #[error("input device not found: {0}")]
    DeviceNotFound(String),
    #[error("no input devices")]
    NoDevices,
    #[error("device disconnected during recording")]
    DeviceLost,
    #[error("microphone access denied: {0}")]
    PermissionDenied(String),
    #[error("recording is not running")]
    NotRecording,
    #[error("recording is already running")]
    AlreadyRecording,
    #[error("could not decode {path}: {reason}")]
    Decode { path: String, reason: String },
    #[error("audio subsystem error: {0}")]
    Backend(String),
}

/// Audio source: the microphone in production, a WAV fixture in tests
///
/// Microphone privacy is a guarantee of the contract, not a property of one implementation:
///
/// - **outside a recording the microphone is off**: the stream opens only in `start` and closes
///   in `stop`, so between phrases MolvAI listens to nothing and the system recording
///   indicator is off
/// - **it turns on fast at the key press**: `start` opens the stream synchronously and returns
///   when the capture already runs — less than 200 ms from the key press to the recording
/// - **it is released right after the phrase**: `stop` closes the stream and returns the
///   recorded audio, and does not keep the stream open "just in case" — less than 500 ms from
///   the key release to the close
///
/// An implementation must release the device on drop too: a phrase can end abnormally, but the
/// microphone is still free after that
pub trait AudioSource: std::fmt::Debug + Send {
    /// Start the capture. `level_tx` gets the RMS level at most about 10 times per second
    fn start(&mut self, level_tx: Option<Sender<f32>>) -> Result<(), AudioError>;
    /// Stop the capture, release the microphone and return everything recorded
    fn stop(&mut self) -> Result<PcmAudio, AudioError>;
    /// Whether the capture runs right now. `false` means the microphone is free
    fn is_recording(&self) -> bool;

    /// The samples that appeared after the previous such call, without stopping the recording
    ///
    /// Streaming processing depends on this: the daemon takes the fresh audio during speech and
    /// gives it to the segmenter. This is a read position, not a removal: [`AudioSource::stop`]
    /// still returns the whole recording, so the caller counts how much it has already
    /// processed. A source that cannot do this returns `None` — then the phrase is processed as a
    /// whole, as before
    fn drain_new_samples(&mut self) -> Option<PcmAudio> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_is_samples_over_rate() {
        let audio = PcmAudio::new(vec![0.0; 32_000], 16_000);
        assert!((audio.duration_secs() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn duration_of_empty_or_zero_rate_is_zero() {
        assert_eq!(PcmAudio::default().duration_secs(), 0.0);
        assert_eq!(PcmAudio::new(vec![1.0; 10], 0).duration_secs(), 0.0);
    }

    #[test]
    fn stereo_is_averaged_into_mono() {
        let interleaved = [1.0, 0.0, 0.5, 0.5, -1.0, 1.0];
        assert_eq!(downmix_to_mono(&interleaved, 2), vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn mono_and_zero_channels_pass_through() {
        let mono = [0.1, 0.2, 0.3];
        assert_eq!(downmix_to_mono(&mono, 1), mono.to_vec());
        assert_eq!(downmix_to_mono(&mono, 0), mono.to_vec());
    }

    #[test]
    fn resample_48k_to_16k_keeps_duration() {
        let audio = PcmAudio::new(vec![0.25; 48_000], 48_000);
        let out = audio.to_16k();
        assert_eq!(out.sample_rate, 16_000);
        assert_eq!(out.samples.len(), 16_000);
        assert!(out.samples.iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let audio = PcmAudio::new(vec![0.1, -0.2, 0.3], 16_000);
        assert_eq!(audio.to_16k(), audio);
    }

    #[test]
    fn resample_interpolates_between_neighbours() {
        // 2 → 4 samples: the middle between 0.0 and 1.0 must become 0.5
        let audio = PcmAudio::new(vec![0.0, 1.0], 2);
        let out = resample_linear(&audio, 4);
        assert_eq!(out.samples.len(), 4);
        assert!((out.samples[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_of_constant_signal_is_its_amplitude() {
        let audio = PcmAudio::new(vec![0.5; 100], 16_000);
        assert!((audio.rms() - 0.5).abs() < 1e-6);
        assert_eq!(PcmAudio::default().rms(), 0.0);
    }
}
