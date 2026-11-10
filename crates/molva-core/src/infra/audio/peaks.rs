// SPDX-License-Identifier: MIT
//! The loudness of a recording over time, read from the file without holding it in memory
//!
//! A whole-file decode keeps every sample: an hour of speech is a buffer of hundreds of megabytes,
//! and the window waits for all of it before it draws a picture. Here the walk of the packets
//! keeps one peak per second, which is a few kilobytes for any length, and the picture is built
//! from those peaks at the end

use std::path::Path;

use crate::domain::audio::AudioError;
use crate::infra::audio::decode::walk_file;

/// The peaks of a file and its length, as the window draws them
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    /// One byte per bucket, loudest at 255
    pub peaks: Vec<u8>,
    pub duration_ms: u64,
}

/// The peaks of one file, one byte per bucket
///
/// Each second of sound is folded into one peak while the file is read, so the memory does not
/// grow with the length, and the buckets then take the loudest second they cover. A silent
/// recording gives zeros rather than a division by zero
pub fn peaks_of_file(path: &Path, buckets: usize) -> Result<Peaks, AudioError> {
    let label = path.display().to_string();
    let mut seconds: Vec<f32> = Vec::new();
    let mut current = 0f32;
    let mut samples_in_second = 0u64;
    let mut rate = 0u32;
    let mut total_samples = 0u64;

    walk_file(path, &mut |samples, sample_rate| {
        rate = sample_rate;
        let per_second = u64::from(sample_rate.max(1));
        total_samples += samples.len() as u64;
        for sample in samples {
            let level = sample.abs();
            if level > current {
                current = level;
            }
            samples_in_second += 1;
            if samples_in_second >= per_second {
                seconds.push(current);
                current = 0.0;
                samples_in_second = 0;
            }
        }
    })?;
    // The last, unfinished second still belongs to the picture
    if samples_in_second > 0 {
        seconds.push(current);
    }
    if total_samples == 0 {
        return Err(AudioError::Decode {
            path: label,
            reason: "the file has no audio".into(),
        });
    }

    let peaks = fold(&seconds, buckets);
    Ok(Peaks {
        peaks,
        duration_ms: if rate == 0 {
            0
        } else {
            (total_samples * 1000) / u64::from(rate)
        },
    })
}

/// One peak per bucket: the loudest second the bucket covers, relative to the loudest of the file
fn fold(seconds: &[f32], buckets: usize) -> Vec<u8> {
    if buckets == 0 || seconds.is_empty() {
        return Vec::new();
    }
    let loudest = seconds.iter().copied().fold(0f32, f32::max);
    if loudest <= f32::EPSILON {
        return vec![0; buckets];
    }
    (0..buckets)
        .map(|bucket| {
            let from = bucket * seconds.len() / buckets;
            let to = ((bucket + 1) * seconds.len() / buckets)
                .max(from + 1)
                .min(seconds.len());
            let peak = seconds[from..to].iter().copied().fold(0f32, f32::max);
            ((peak / loudest) * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quiet_half_comes_out_lower_than_a_loud_one() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("halves.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("a wav");
        for _ in 0..8_000 {
            writer.write_sample(30_000i16).expect("a sample");
        }
        for _ in 0..8_000 {
            writer.write_sample(3_000i16).expect("a sample");
        }
        writer.finalize().expect("a finished wav");

        let peaks = peaks_of_file(&path, 2).expect("a picture");
        assert_eq!(peaks.peaks.len(), 2);
        assert_eq!(peaks.peaks[0], 255);
        assert!(peaks.peaks[1] < peaks.peaks[0], "{:?}", peaks.peaks);
        assert_eq!(peaks.duration_ms, 2_000);
    }

    #[test]
    fn a_silent_file_gives_a_flat_line_and_keeps_its_length() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("silence.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).expect("a wav");
        for _ in 0..8_000 {
            writer.write_sample(0i16).expect("a sample");
        }
        writer.finalize().expect("a finished wav");

        let peaks = peaks_of_file(&path, 4).expect("a picture");
        assert_eq!(peaks.peaks, vec![0, 0, 0, 0]);
        assert_eq!(peaks.duration_ms, 1_000);
    }

    #[test]
    fn a_file_without_sound_is_an_error_with_a_reason() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("empty.wav");
        std::fs::write(&path, b"").expect("an empty file");
        let err = peaks_of_file(&path, 4).expect_err("no picture");
        assert!(err.to_string().contains("empty"), "{err}");
    }
}
