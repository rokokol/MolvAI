// SPDX-License-Identifier: MIT
//! Sound cues of the recording through cpal: a short sine tone to the default output device
//!
//! This implements the criteria "a sound signal at the start and at the end of the recording"
//! and "the signal can be turned off by a setting": [`build_sound_cue`] returns, by
//! `audio.sounds`, either [`CpalSoundCue`] or [`NullSoundCue`], which stays silent. The volume
//! comes from `audio.sound_volume`
//!
//! The sound must not play synchronously: the daemon calls `play` from the control thread right
//! before it opens the microphone, and a wait of eighty milliseconds would use up the reaction
//! budget for the key press. So each signal goes to a separate short-lived thread

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use tracing::{debug, warn};

use crate::config::AudioConfig;
use crate::domain::sound::{CueKind, SoundCue};

/// Length of the smooth fade-in and fade-out: without them a short tone clicks at the edges
const FADE_MS: f32 = 8.0;

/// Silence: `audio.sounds = false`
///
/// It is a separate type, not an `Option<Arc<dyn SoundCue>>` on the caller side: a turned-off
/// sound must not need a branch in the daemon, or someone will forget one of the branches
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSoundCue;

impl SoundCue for NullSoundCue {
    fn id(&self) -> &'static str {
        "null"
    }

    fn play(&self, _kind: CueKind) {}
}

/// Sine-tone cues through the default output device
#[derive(Debug, Clone, Copy)]
pub struct CpalSoundCue {
    volume: f32,
}

impl CpalSoundCue {
    /// `volume` is a fraction of the full scale, clamped to the range 0…1
    pub fn new(volume: f32) -> Self {
        Self {
            volume: volume.clamp(0.0, 1.0),
        }
    }
}

impl SoundCue for CpalSoundCue {
    fn id(&self) -> &'static str {
        "cpal"
    }

    fn play(&self, kind: CueKind) {
        let volume = self.volume;
        if volume <= 0.0 {
            return;
        }
        // The sound must not delay the opening of the microphone: play it aside
        let spawned = std::thread::Builder::new()
            .name("molva-sound".into())
            .spawn(move || {
                if let Err(err) = play_tone(kind, volume) {
                    debug!(cue = kind.label(), error = %err, "cue not played");
                }
            });
        if let Err(err) = spawned {
            warn!(error = %err, "could not start the thread of the sound cue");
        }
    }
}

/// Player by the settings: a turned-off sound is a `NullSoundCue`, not a skipped call
pub fn build_sound_cue(audio: &AudioConfig) -> Arc<dyn SoundCue> {
    if !audio.sounds {
        return Arc::new(NullSoundCue);
    }
    Arc::new(CpalSoundCue::new(audio.sound_volume))
}

/// One sample of a sine wave with smooth edges
///
/// It is a pure function: tests check the shape of the signal without a sound card
pub fn tone_sample(index: usize, total: usize, freq_hz: f32, sample_rate: f32, volume: f32) -> f32 {
    if total == 0 || sample_rate <= 0.0 {
        return 0.0;
    }
    let t = index as f32 / sample_rate;
    let wave = (t * freq_hz * std::f32::consts::TAU).sin();
    let fade = (sample_rate * FADE_MS / 1000.0).max(1.0);
    let from_start = index as f32;
    let to_end = (total - index.min(total)) as f32;
    let envelope = (from_start / fade).min(to_end / fade).clamp(0.0, 1.0);
    wave * envelope * volume
}

/// How many samples the cue takes at this sample rate
pub fn tone_len(kind: CueKind, sample_rate: u32) -> usize {
    (u64::from(sample_rate) * u64::from(kind.duration_ms()) / 1000) as usize
}

/// Open an output stream, play the tone and close the stream
fn play_tone(kind: CueKind, volume: f32) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "no output device".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("output configuration is unavailable: {e}"))?;
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let config: StreamConfig = supported.config();

    let tone = Tone::new(kind, sample_rate, volume);

    let stream = match sample_format {
        SampleFormat::F32 => build_output::<f32>(&device, &config, channels, tone),
        SampleFormat::I16 => build_output::<i16>(&device, &config, channels, tone),
        SampleFormat::I32 => build_output::<i32>(&device, &config, channels, tone),
        SampleFormat::U16 => build_output::<u16>(&device, &config, channels, tone),
        SampleFormat::U8 => build_output::<u8>(&device, &config, channels, tone),
        SampleFormat::F64 => build_output::<f64>(&device, &config, channels, tone),
        other => Err(format!("output format is not supported: {other:?}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("stream did not start: {e}"))?;
    // The stream lives as long as the tone sounds, plus a margin for the device buffer
    std::thread::sleep(Duration::from_millis(u64::from(kind.duration_ms()) + 60));
    drop(stream);
    Ok(())
}

/// Tone generator: it remembers how many samples it has returned, and stays silent after the end
/// of the cue
#[derive(Debug, Clone, Copy)]
struct Tone {
    index: usize,
    total: usize,
    freq_hz: f32,
    sample_rate: f32,
    volume: f32,
}

impl Tone {
    fn new(kind: CueKind, sample_rate: u32, volume: f32) -> Self {
        Self {
            index: 0,
            total: tone_len(kind, sample_rate),
            freq_hz: kind.frequency_hz(),
            sample_rate: sample_rate as f32,
            volume,
        }
    }

    fn next_sample(&mut self) -> f32 {
        if self.index > self.total {
            return 0.0;
        }
        let sample = tone_sample(
            self.index,
            self.total,
            self.freq_hz,
            self.sample_rate,
            self.volume,
        );
        self.index += 1;
        sample
    }
}

/// Build an output stream for a given sample type
fn build_output<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    mut tone: Tone,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    // Each frame is `channels` equal samples: the tone is mono, so there is nothing to pan
    let channels = channels.max(1) as usize;
    device
        .build_output_stream::<T, _, _>(
            *config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    let value = tone.next_sample();
                    for sample in frame.iter_mut() {
                        *sample = T::from_sample(value);
                    }
                }
            },
            move |err| warn!(error = %err, "sound output stream error"),
            None,
        )
        .map_err(|e| format!("output stream did not open: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_switches_the_cues_off() {
        // Criterion AG-06: `audio.sounds = false` means no sound at all, not a quiet one
        let mut audio = AudioConfig {
            sounds: false,
            ..AudioConfig::default()
        };
        assert_eq!(build_sound_cue(&audio).id(), "null");
        // The silent implementation must be safe with no sound card
        build_sound_cue(&audio).play(CueKind::RecordStart);

        audio.sounds = true;
        assert_eq!(build_sound_cue(&audio).id(), "cpal");
    }

    #[test]
    fn zero_volume_plays_nothing() {
        let cue = CpalSoundCue::new(0.0);
        // Zero volume does not open the output device: the test does not touch the hardware
        cue.play(CueKind::RecordStop);
        assert_eq!(
            CpalSoundCue::new(2.0).volume,
            1.0,
            "the volume is limited to 1.0"
        );
    }

    #[test]
    fn the_tone_starts_and_ends_at_zero() {
        let total = tone_len(CueKind::RecordStart, 48_000);
        assert!(total > 0);
        assert_eq!(tone_sample(0, total, 880.0, 48_000.0, 0.4), 0.0);
        let last = tone_sample(total, total, 880.0, 48_000.0, 0.4);
        assert!(last.abs() < 1e-6, "{last}");
    }

    #[test]
    fn the_tone_never_exceeds_the_configured_volume() {
        let total = tone_len(CueKind::RecordStop, 44_100);
        let peak = (0..total)
            .map(|i| tone_sample(i, total, 660.0, 44_100.0, 0.4).abs())
            .fold(0.0f32, f32::max);
        assert!(peak <= 0.4 + 1e-6, "{peak}");
        assert!(peak > 0.3, "the cue must be audible: {peak}");
    }

    #[test]
    fn the_length_of_a_cue_matches_its_duration() {
        assert_eq!(tone_len(CueKind::RecordStart, 16_000), 16_000 * 80 / 1000);
        assert_eq!(tone_len(CueKind::Error, 16_000), 16_000 * 160 / 1000);
    }
}
