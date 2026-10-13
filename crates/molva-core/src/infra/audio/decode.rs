// SPDX-License-Identifier: MIT
//! Decoding of audio files through symphonia
//!
//! Any supported container becomes mono `f32` with the *native* sample rate: converting to
//! 16 kHz is the job of the caller (`PcmAudio::to_16k`), because the benchmark and the
//! statistics need the original duration
//!
//! All failures are one error, `AudioError::Decode { path, reason }`, with a human reason: from
//! the text the user understands what is wrong with the file, and does not read a library trace

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use crate::domain::audio::{downmix_to_mono, AudioError, PcmAudio};

/// Extensions that the CLI treats as audio files when it walks a directory
///
/// The list is wider than the set of codecs on purpose: if the container is known but the codec
/// inside is unknown, the user gets a clear error instead of a silent skip of the file
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "wave", "mp3", "ogg", "oga", "flac", "m4a", "mp4", "aac", "m4b",
];

/// Alias of the path for stdin in error messages
pub const STDIN_LABEL: &str = "<stdin>";

/// Whether the file name looks like audio from the supported list
pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

fn decode_error(label: &str, reason: impl Into<String>) -> AudioError {
    AudioError::Decode {
        path: label.to_string(),
        reason: reason.into(),
    }
}

/// Read a whole file and return a mono signal with the native sample rate
pub fn decode_file(path: &Path) -> Result<PcmAudio, AudioError> {
    let label = path.display().to_string();
    let meta = std::fs::metadata(path)
        .map_err(|e| decode_error(&label, format!("could not open the file: {e}")))?;
    if meta.is_dir() {
        return Err(decode_error(
            &label,
            "this is a directory, not an audio file",
        ));
    }
    if meta.len() == 0 {
        return Err(decode_error(&label, "the file is empty (0 bytes)"));
    }
    let file = File::open(path)
        .map_err(|e| decode_error(&label, format!("could not open the file: {e}")))?;

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    decode_source(Box::new(file), &hint, &label)
}

/// Decode any source: used for stdin and tests
///
/// `extension` is a format hint, if it is known (`Some("wav")`); without it symphonia detects
/// the format by the signature at the start of the stream
pub fn decode_reader(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
    label: &str,
) -> Result<PcmAudio, AudioError> {
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    decode_source(source, &hint, label)
}

/// Read the whole stdin into memory and decode it
///
/// The stream cannot be rewound, and symphonia moves back in the buffer when it detects the
/// format, so the bytes are first collected in a `Cursor`
pub fn decode_stdin(extension: Option<&str>) -> Result<PcmAudio, AudioError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|e| decode_error(STDIN_LABEL, format!("could not read the stream: {e}")))?;
    if bytes.is_empty() {
        return Err(decode_error(STDIN_LABEL, "no data on the input (0 bytes)"));
    }
    decode_reader(Box::new(Cursor::new(bytes)), extension, STDIN_LABEL)
}

fn decode_source(
    source: Box<dyn MediaSource>,
    hint: &Hint,
    label: &str,
) -> Result<PcmAudio, AudioError> {
    let stream = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| decode_error(label, format!("format not recognized: {e}")))?;

    let (track_id, params) = format
        .default_track(TrackType::Audio)
        .and_then(|track| Some((track.id, track.codec_params.as_ref()?.audio()?.clone())))
        .ok_or_else(|| decode_error(label, "the file has no audio track"))?;
    // gapless removes the service samples of the encoder (mp3 delay and AAC priming),
    // or the duration of the file differs visibly from the original
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default().gapless(true))
        .map_err(|e| decode_error(label, format!("codec is not supported: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut interleaved: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A truncated file ends with the io error UnexpectedEof: everything that was
            // decoded before the break stays in the result
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(decode_error(
                    label,
                    "the stream changes parameters on the fly — such a file is not supported",
                ))
            }
            Err(e) => return Err(decode_error(label, format!("the file is damaged: {e}"))),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(frames) => {
                let spec = frames.spec();
                sample_rate = spec.rate();
                let channels = spec.channels().count() as u16;
                frames.copy_to_vec_interleaved(&mut interleaved);
                mono.extend_from_slice(&downmix_to_mono(&interleaved, channels));
            }
            // A bad packet in the middle of the file is skipped: the rest of the recording is still
            // useful
            Err(SymphoniaError::DecodeError(_)) => {}
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(e) => return Err(decode_error(label, format!("decoding error: {e}"))),
        }
    }

    if mono.is_empty() {
        return Err(decode_error(label, "the file has no audio"));
    }
    if sample_rate == 0 {
        return Err(decode_error(label, "unknown sample rate"));
    }
    Ok(PcmAudio::new(mono, sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    /// Stereo WAV: the left channel is positive, the right channel is negative, so the mono result
    /// is zero
    fn write_stereo_wav(path: &Path, rate: u32, secs: f32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let frames = (rate as f32 * secs) as usize;
        for _ in 0..frames {
            writer.write_sample(16_000i16).unwrap();
            writer.write_sample(-16_000i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn stereo_wav_becomes_mono_with_native_rate_and_duration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stereo.wav");
        write_stereo_wav(&path, 44_100, 0.5);

        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.samples.len(), 22_050);
        assert!((audio.duration_secs() - 0.5).abs() < 0.01);
        // The channels are in antiphase: averaging gives silence, so the downmix really happened
        assert!(audio.rms() < 1e-3, "rms = {}", audio.rms());
    }

    #[test]
    fn mono_wav_keeps_samples_as_is() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mono.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..16_000 {
            writer.write_sample(8_000i16).unwrap();
        }
        writer.finalize().unwrap();

        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples.len(), 16_000);
        assert!((audio.rms() - 8_000.0 / 32_768.0).abs() < 1e-3);
    }

    #[test]
    fn empty_file_is_rejected_with_reason() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("empty.wav");
        std::fs::write(&path, b"").unwrap();

        let error = decode_file(&path).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("0 bytes"), "{text}");
        assert!(text.contains("empty.wav"), "{text}");
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("broken.wav");
        std::fs::write(&path, b"RIFFxxxxWAVEnot-a-real-header-at-all").unwrap();

        let error = decode_file(&path).unwrap_err();
        assert!(matches!(error, AudioError::Decode { .. }), "{error}");
    }

    #[test]
    fn missing_file_is_rejected() {
        let error = decode_file(Path::new("/nonexistent/molva/none.wav")).unwrap_err();
        assert!(
            error.to_string().contains("could not open the file"),
            "{error}"
        );
    }

    #[test]
    fn directory_is_not_an_audio_file() {
        let directory = tempfile::tempdir().unwrap();
        let error = decode_file(directory.path()).unwrap_err();
        assert!(error.to_string().contains("directory"), "{error}");
    }

    /// The fixtures are a 440 Hz tone of exactly 1 s (see `tests/fixtures/README.md`)
    fn assert_fixture_is_one_second(name: &str) {
        let audio = decode_file(&fixture(name)).unwrap();
        let secs = audio.duration_secs();
        assert!(
            (secs - 1.0).abs() <= 0.05,
            "{name}: duration {secs} s instead of 1.0 ±5 %"
        );
        // The tone is recorded at about −18 dBFS; the threshold separates the signal from a zero
        // buffer
        assert!(
            audio.rms() > 0.01,
            "{name}: the tone decoded as silence (rms {})",
            audio.rms()
        );
    }

    #[test]
    fn mp3_fixture_decodes() {
        assert_fixture_is_one_second("tone.mp3");
    }

    #[test]
    fn ogg_vorbis_fixture_decodes() {
        assert_fixture_is_one_second("tone.ogg");
    }

    #[test]
    fn flac_fixture_decodes() {
        assert_fixture_is_one_second("tone.flac");
    }

    #[test]
    fn m4a_aac_fixture_decodes() {
        assert_fixture_is_one_second("tone.m4a");
    }

    #[test]
    fn stereo_fixture_is_downmixed_to_single_channel() {
        // The mp3 fixture is recorded in two channels; the output is one stream of samples at the
        // file sample rate
        let audio = decode_file(&fixture("tone.mp3")).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert!(audio.samples.len() as f32 / 44_100.0 > 0.9);
    }

    #[test]
    fn speech_fixtures_decode_at_native_16k_mono() {
        for (name, expected) in [("privet_ru.wav", 4.46f32), ("hello_en.wav", 4.16f32)] {
            let audio = decode_file(&fixture(name)).unwrap();
            assert_eq!(audio.sample_rate, 16_000, "{name}");
            let secs = audio.duration_secs();
            assert!(
                (secs - expected).abs() <= expected * 0.05,
                "{name}: duration {secs} s instead of {expected} ±5 %"
            );
            assert!(
                audio.rms() > 0.001,
                "{name}: the recording decoded as silence"
            );
        }
    }

    #[test]
    fn reader_without_extension_hint_is_probed_by_signature() {
        let bytes = std::fs::read(fixture("tone.flac")).unwrap();
        let audio = decode_reader(Box::new(Cursor::new(bytes)), None, "stream").unwrap();
        assert!((audio.duration_secs() - 1.0).abs() <= 0.05);
    }

    #[test]
    fn reader_reports_label_in_error() {
        let error =
            decode_reader(Box::new(Cursor::new(vec![0u8; 64])), None, "stream").unwrap_err();
        assert!(error.to_string().contains("stream"), "{error}");
    }

    #[test]
    fn extension_filter_matches_known_audio_only() {
        assert!(is_supported_audio(Path::new("a/b.WAV")));
        assert!(is_supported_audio(Path::new("a/b.m4a")));
        assert!(!is_supported_audio(Path::new("a/b.txt")));
        assert!(!is_supported_audio(Path::new("a/b")));
    }
}
