// SPDX-License-Identifier: MIT
//! Декодирование аудиофайлов через symphonia.
//!
//! Любой поддерживаемый контейнер сводится к моно `f32` с *родной* частотой дискретизации:
//! приводить к 16 кГц — дело вызывающего (`PcmAudio::to_16k`), потому что бенчу и статистике
//! нужна исходная длительность.
//!
//! Все отказы — одна ошибка `AudioError::Decode { path, reason }` с человеческой причиной:
//! пользователь по тексту понимает, что не так с файлом, а не читает трейс библиотеки.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::domain::audio::{downmix_to_mono, AudioError, PcmAudio};

/// Расширения, которые CLI считает аудиофайлами при обходе каталога.
///
/// Список сознательно шире набора кодеков: если контейнер знаком, а кодек внутри неизвестен,
/// пользователь получит внятную ошибку вместо молчаливого пропуска файла.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "wave", "mp3", "ogg", "oga", "flac", "m4a", "mp4", "aac", "m4b",
];

/// Псевдоним пути для stdin в сообщениях об ошибках.
pub const STDIN_LABEL: &str = "<stdin>";

/// Похоже ли имя файла на аудио из поддерживаемого списка.
pub fn is_supported_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

fn decode_error(label: &str, reason: impl Into<String>) -> AudioError {
    AudioError::Decode {
        path: label.to_string(),
        reason: reason.into(),
    }
}

/// Прочитать файл целиком и вернуть моно-сигнал с родной частотой.
pub fn decode_file(path: &Path) -> Result<PcmAudio, AudioError> {
    let label = path.display().to_string();
    let meta = std::fs::metadata(path)
        .map_err(|e| decode_error(&label, format!("не удалось открыть файл: {e}")))?;
    if meta.is_dir() {
        return Err(decode_error(&label, "это каталог, а не аудиофайл"));
    }
    if meta.len() == 0 {
        return Err(decode_error(&label, "файл пуст (0 байт)"));
    }
    let file = File::open(path)
        .map_err(|e| decode_error(&label, format!("не удалось открыть файл: {e}")))?;

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    decode_source(Box::new(file), hint, &label)
}

/// Декодировать произвольный источник: используется для stdin и тестов.
///
/// `extension` — подсказка формата, если она известна (`Some("wav")`); без неё symphonia
/// определяет формат по сигнатуре в начале потока.
pub fn decode_reader(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
    label: &str,
) -> Result<PcmAudio, AudioError> {
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    decode_source(source, hint, label)
}

/// Прочитать stdin целиком в память и декодировать.
///
/// Поток нельзя перемотать, а symphonia при определении формата ходит по буферу назад,
/// поэтому байты сначала собираются в `Cursor`.
pub fn decode_stdin(extension: Option<&str>) -> Result<PcmAudio, AudioError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|e| decode_error(STDIN_LABEL, format!("не удалось прочитать поток: {e}")))?;
    if bytes.is_empty() {
        return Err(decode_error(STDIN_LABEL, "на входе нет данных (0 байт)"));
    }
    decode_reader(Box::new(Cursor::new(bytes)), extension, STDIN_LABEL)
}

fn decode_source(
    source: Box<dyn MediaSource>,
    hint: Hint,
    label: &str,
) -> Result<PcmAudio, AudioError> {
    let stream = MediaSourceStream::new(source, Default::default());
    // enable_gapless убирает служебные отсчёты кодировщика (задержка mp3 и priming AAC),
    // иначе длительность файла заметно расходится с исходной.
    let format_opts = FormatOptions {
        enable_gapless: true,
        ..FormatOptions::default()
    };
    let probed = symphonia::default::get_probe()
        .format(&hint, stream, &format_opts, &MetadataOptions::default())
        .map_err(|e| decode_error(label, format!("формат не распознан: {e}")))?;
    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| decode_error(label, "в файле нет звуковой дорожки"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| decode_error(label, format!("кодек не поддерживается: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut buffer: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // Конец потока symphonia сообщает как io-ошибку UnexpectedEof.
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => {
                return Err(decode_error(
                    label,
                    "поток меняет параметры на ходу — такой файл не поддерживается",
                ))
            }
            Err(e) => return Err(decode_error(label, format!("файл повреждён: {e}"))),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                sample_rate = spec.rate;
                let channels = spec.channels.count() as u16;
                let buf = buffer.get_or_insert_with(|| {
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, spec)
                });
                buf.copy_interleaved_ref(decoded);
                mono.extend_from_slice(&downmix_to_mono(buf.samples(), channels));
            }
            // Битый пакет посреди файла — пропускаем: остальная запись всё ещё полезна.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(e) => return Err(decode_error(label, format!("ошибка декодирования: {e}"))),
        }
    }

    if mono.is_empty() {
        return Err(decode_error(label, "в файле нет звука"));
    }
    if sample_rate == 0 {
        return Err(decode_error(label, "неизвестная частота дискретизации"));
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

    /// Стерео-WAV: левый канал — единицы, правый — минус единицы, чтобы моно вышло нулём.
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        write_stereo_wav(&path, 44_100, 0.5);

        let audio = decode_file(&path).unwrap();
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.samples.len(), 22_050);
        assert!((audio.duration_secs() - 0.5).abs() < 0.01);
        // Каналы противофазны: усреднение даёт тишину, значит сведение действительно было.
        assert!(audio.rms() < 1e-3, "rms = {}", audio.rms());
    }

    #[test]
    fn mono_wav_keeps_samples_as_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mono.wav");
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.wav");
        std::fs::write(&path, b"").unwrap();

        let err = decode_file(&path).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("0 байт"), "{text}");
        assert!(text.contains("empty.wav"), "{text}");
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.wav");
        std::fs::write(&path, b"RIFFxxxxWAVEnot-a-real-header-at-all").unwrap();

        let err = decode_file(&path).unwrap_err();
        assert!(matches!(err, AudioError::Decode { .. }), "{err}");
    }

    #[test]
    fn missing_file_is_rejected() {
        let err = decode_file(Path::new("/nonexistent/molva/none.wav")).unwrap_err();
        assert!(err.to_string().contains("не удалось открыть файл"), "{err}");
    }

    #[test]
    fn directory_is_not_an_audio_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = decode_file(dir.path()).unwrap_err();
        assert!(err.to_string().contains("каталог"), "{err}");
    }

    /// Фикстуры — тон 440 Гц длительностью ровно 1 с (см. `tests/fixtures/README.md`).
    fn assert_fixture_is_one_second(name: &str) {
        let audio = decode_file(&fixture(name)).unwrap();
        let secs = audio.duration_secs();
        assert!(
            (secs - 1.0).abs() <= 0.05,
            "{name}: длительность {secs} с вместо 1.0 ±5 %"
        );
        // Тон записан примерно на −18 dBFS; порог отделяет сигнал от нулевого буфера.
        assert!(
            audio.rms() > 0.01,
            "{name}: тон декодировался как тишина (rms {})",
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
        // mp3-фикстура записана в два канала; на выходе один поток отсчётов с частотой файла.
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
                "{name}: длительность {secs} с вместо {expected} ±5 %"
            );
            assert!(
                audio.rms() > 0.001,
                "{name}: запись декодировалась как тишина"
            );
        }
    }

    #[test]
    fn reader_without_extension_hint_is_probed_by_signature() {
        let bytes = std::fs::read(fixture("tone.flac")).unwrap();
        let audio = decode_reader(Box::new(Cursor::new(bytes)), None, "поток").unwrap();
        assert!((audio.duration_secs() - 1.0).abs() <= 0.05);
    }

    #[test]
    fn reader_reports_label_in_error() {
        let err = decode_reader(Box::new(Cursor::new(vec![0u8; 64])), None, "поток").unwrap_err();
        assert!(err.to_string().contains("поток"), "{err}");
    }

    #[test]
    fn extension_filter_matches_known_audio_only() {
        assert!(is_supported_audio(Path::new("a/b.WAV")));
        assert!(is_supported_audio(Path::new("a/b.m4a")));
        assert!(!is_supported_audio(Path::new("a/b.txt")));
        assert!(!is_supported_audio(Path::new("a/b")));
    }
}
