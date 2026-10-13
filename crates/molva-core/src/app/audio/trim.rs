// SPDX-License-Identifier: MIT
//! Trimming of silence at the edges of a recording
//!
//! The user presses the hotkey before speech starts and releases it after speech stops, so
//! fractions of a second of silence remain at the edges. Whisper tends to hallucinate on such
//! silence ("Продолжение следует…", "Субтитры сделал DimaTorzok"), and WPM is computed from
//! the audio duration. So the edges are cut and the inner pauses stay as they are: a pause
//! inside a phrase is part of speech, and cutting it would join words and inflate the pace

use crate::config::AudioConfig;
use crate::domain::audio::PcmAudio;

/// Margin that remains before the first sound and after the last sound
///
/// Zero would cut the attack of the first consonant, and half a second would bring back the
/// silence that the trimming was meant to remove
///
/// This margin also gives the guarantee "the first character of a phrase is not lost" (AM-04):
/// the first two hundred milliseconds of a recording are never discarded. So a plosive
/// consonant at the start of a word ("паспорт", "ключ") reaches whisper whole, not by half
pub const DEFAULT_KEEP_MS: u32 = 200;

/// Analysis window: 20 ms balances response to short sounds against resistance to noise
const WINDOW_MS: u32 = 20;

/// Level that counts as "absolute zero": the logarithm of zero is not defined
pub const SILENCE_FLOOR_DB: f32 = -120.0;

/// Peak level of the signal in dBFS (0 dB is the maximum amplitude 1.0)
///
/// An empty buffer and absolute silence give [`SILENCE_FLOOR_DB`]
pub fn peak_db(audio: &PcmAudio) -> f32 {
    let peak = audio
        .samples
        .iter()
        .fold(0.0_f32, |acc, s| acc.max(s.abs()));
    amplitude_to_db(peak)
}

/// Whether the recording has at least one window louder than the threshold
///
/// The threshold is set in dBFS (in the config as `audio.silence_threshold_db`, default −45)
pub fn is_silent(audio: &PcmAudio, threshold_db: f32) -> bool {
    loudest_window_db(audio) < threshold_db
}

/// Trim silence at the edges, and keep `keep_ms` of margin before the first sound and after the
/// last sound
///
/// Returns an empty buffer with the same sample rate if no window is louder than the threshold.
/// Inner pauses are not touched
pub fn trim_silence(audio: &PcmAudio, threshold_db: f32, keep_ms: u32) -> PcmAudio {
    let window = window_len(audio.sample_rate);
    if audio.samples.is_empty() || window == 0 {
        return PcmAudio::new(Vec::new(), audio.sample_rate);
    }

    let voiced: Vec<usize> = audio
        .samples
        .chunks(window)
        .enumerate()
        .filter(|(_, chunk)| amplitude_to_db(rms(chunk)) >= threshold_db)
        .map(|(index, _)| index)
        .collect();

    let (Some(&first), Some(&last)) = (voiced.first(), voiced.last()) else {
        return PcmAudio::new(Vec::new(), audio.sample_rate);
    };

    let keep = (u64::from(audio.sample_rate) * u64::from(keep_ms) / 1000) as usize;
    let start = (first * window).saturating_sub(keep);
    let end = ((last + 1) * window + keep).min(audio.samples.len());

    PcmAudio::new(audio.samples[start..end].to_vec(), audio.sample_rate)
}

/// Trimming by the user settings
///
/// With `audio.trim_silence = false` the recording is returned as it is. Inner pauses are never
/// cut, so `audio.vad_min_pause_ms` holds by construction: a pause of any length inside a
/// phrase stays in place (E-01/02)
pub fn trim_for_config(audio: &PcmAudio, cfg: &AudioConfig) -> PcmAudio {
    if !cfg.trim_silence {
        return audio.clone();
    }
    trim_silence(audio, cfg.silence_threshold_db, DEFAULT_KEEP_MS)
}

/// Level of the loudest window; for an empty buffer it is [`SILENCE_FLOOR_DB`]
fn loudest_window_db(audio: &PcmAudio) -> f32 {
    let window = window_len(audio.sample_rate);
    if audio.samples.is_empty() || window == 0 {
        return SILENCE_FLOOR_DB;
    }
    audio
        .samples
        .chunks(window)
        .map(|chunk| amplitude_to_db(rms(chunk)))
        .fold(SILENCE_FLOOR_DB, f32::max)
}

/// Length of the analysis window in samples; a zero sample rate gives 0, which the caller reads
/// as "no data"
fn window_len(sample_rate: u32) -> usize {
    (u64::from(sample_rate) * u64::from(WINDOW_MS) / 1000) as usize
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Amplitude 0.0..=1.0 → dBFS, with a floor at [`SILENCE_FLOOR_DB`]
fn amplitude_to_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return SILENCE_FLOOR_DB;
    }
    (20.0 * amplitude.log10()).max(SILENCE_FLOOR_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Segment of a sine of the given amplitude: a steady signal with a predictable RMS
    fn tone(ms: u32, amplitude: f32) -> Vec<f32> {
        let n = (u64::from(RATE) * u64::from(ms) / 1000) as usize;
        (0..n).map(|i| amplitude * (i as f32 * 0.3).sin()).collect()
    }

    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (u64::from(RATE) * u64::from(ms) / 1000) as usize]
    }

    fn audio(samples: Vec<f32>) -> PcmAudio {
        PcmAudio::new(samples, RATE)
    }

    #[test]
    fn silence_trims_to_nothing() {
        let out = trim_silence(&audio(silence(1000)), -45.0, 100);
        assert!(out.samples.is_empty());
        assert_eq!(out.sample_rate, RATE);
    }

    #[test]
    fn empty_input_stays_empty() {
        let out = trim_silence(&PcmAudio::new(Vec::new(), RATE), -45.0, 100);
        assert!(out.samples.is_empty());
    }

    #[test]
    fn leading_and_trailing_silence_are_cut() {
        let mut samples = silence(500);
        samples.extend(tone(300, 0.5));
        samples.extend(silence(500));
        let out = trim_silence(&audio(samples), -45.0, 0);

        // The speech remains, ± one analysis window (20 ms)
        let expected = (RATE as f32 * 0.3) as usize;
        let slack = window_len(RATE) * 2;
        assert!(
            out.samples.len() <= expected + slack,
            "not trimmed: {} samples where ~{expected} expected",
            out.samples.len()
        );
        assert!(
            out.samples.len() + slack >= expected,
            "the speech itself was cut"
        );
    }

    #[test]
    fn keep_ms_leaves_padding_before_speech() {
        let mut samples = silence(500);
        samples.extend(tone(300, 0.5));
        samples.extend(silence(500));
        let keep_ms = 100;
        let out = trim_silence(&audio(samples), -45.0, keep_ms);

        let expected = (RATE as f32 * 0.3) as usize + 2 * (RATE as usize * keep_ms as usize / 1000);
        let slack = window_len(RATE) * 2;
        assert!(
            out.samples.len() + slack >= expected,
            "the keep_ms margin was not kept: {} samples",
            out.samples.len()
        );
    }

    #[test]
    fn inner_pause_is_not_removed() {
        let mut samples = tone(200, 0.5);
        samples.extend(silence(800));
        samples.extend(tone(200, 0.5));
        let total = samples.len();
        let out = trim_silence(&audio(samples), -45.0, 0);

        // The pause inside the phrase stays: the length barely changed
        assert!(
            out.samples.len() * 100 >= total * 95,
            "the inner pause was cut: {} of {total}",
            out.samples.len()
        );
    }

    #[test]
    fn trimmed_start_keeps_first_speech_sample() {
        let mut samples = silence(500);
        let speech = tone(300, 0.5);
        let first_loud = speech
            .iter()
            .position(|s| s.abs() > 0.4)
            .expect("the tone has a loud sample");
        samples.extend(speech.clone());
        let out = trim_silence(&audio(samples), -45.0, 20);

        // The start of the speech is not eaten: a loud sample stays in the first 100 ms of the
        // result
        let head = &out.samples[..out.samples.len().min(RATE as usize / 10)];
        assert!(
            head.iter().any(|s| s.abs() > 0.4),
            "the start of the speech is lost (the first loud sample was at {first_loud})"
        );
    }

    #[test]
    fn quiet_noise_below_threshold_is_treated_as_silence() {
        let out = trim_silence(&audio(tone(1000, 0.001)), -45.0, 0);
        assert!(
            out.samples.is_empty(),
            "noise at −60 dB was taken for speech"
        );
    }

    #[test]
    fn is_silent_distinguishes_speech_from_room_tone() {
        assert!(is_silent(&audio(silence(500)), -45.0));
        assert!(is_silent(&audio(tone(500, 0.001)), -45.0));
        assert!(!is_silent(&audio(tone(500, 0.5)), -45.0));
    }

    #[test]
    fn disabled_trimming_returns_the_recording_untouched() {
        let cfg = AudioConfig {
            trim_silence: false,
            ..AudioConfig::default()
        };
        let input = audio(silence(1000));

        assert_eq!(trim_for_config(&input, &cfg), input);
    }

    #[test]
    fn config_threshold_is_used_for_trimming() {
        let mut samples = silence(500);
        samples.extend(tone(300, 0.5));
        let input = audio(samples);

        // Threshold −45 dB: the speech is louder, so we keep it
        let normal = AudioConfig::default();
        assert!(!trim_for_config(&input, &normal).samples.is_empty());

        // Threshold 0 dB: nothing is louder, so the whole recording counts as silence
        let deaf = AudioConfig {
            silence_threshold_db: 0.0,
            ..AudioConfig::default()
        };
        assert!(trim_for_config(&input, &deaf).samples.is_empty());
    }

    #[test]
    fn peak_db_of_full_scale_is_zero() {
        assert!((peak_db(&audio(vec![1.0, -1.0])) - 0.0).abs() < 1e-3);
        assert_eq!(peak_db(&audio(Vec::new())), SILENCE_FLOOR_DB);
        assert!(peak_db(&audio(vec![0.5])) < 0.0);
    }
}
