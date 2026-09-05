// SPDX-License-Identifier: MIT
//! Обрезка тишины по краям записи.
//!
//! Пользователь нажимает хоткей раньше, чем начинает говорить, и отпускает позже, чем замолчал:
//! на краях остаются доли секунды тишины. Whisper на такой тишине склонен галлюцинировать
//! («Продолжение следует…», «Субтитры сделал DimaTorzok»), а WPM считается по длительности
//! аудио — поэтому края режутся, а внутренние паузы сохраняются как есть: пауза внутри реплики
//! это часть речи, и вырезать её значит склеить слова и завысить темп.

use crate::config::AudioConfig;
use crate::domain::audio::PcmAudio;

/// Запас, который остаётся до первого и после последнего звука.
///
/// Ноль обрезал бы атаку первого согласного, а полсекунды вернули бы тишину, ради которой всё и
/// затевалось.
///
/// Отсюда же гарантия «первый символ реплики не теряется» (AM-04): двести миллисекунд в начале
/// записи не выбрасываются никогда, поэтому взрывной согласный в начале слова («паспорт»,
/// «ключ») доезжает до whisper целиком, а не половиной.
pub const DEFAULT_KEEP_MS: u32 = 200;

/// Окно анализа: 20 мс — компромисс между реакцией на короткие звуки и устойчивостью к шуму.
const WINDOW_MS: u32 = 20;

/// Уровень, который считаем «абсолютным нулём»: логарифм от нуля не определён.
pub const SILENCE_FLOOR_DB: f32 = -120.0;

/// Пиковый уровень сигнала в дБFS (0 дБ — максимальная амплитуда 1.0).
///
/// Пустой буфер и абсолютная тишина дают [`SILENCE_FLOOR_DB`].
pub fn peak_db(audio: &PcmAudio) -> f32 {
    let peak = audio
        .samples
        .iter()
        .fold(0.0_f32, |acc, s| acc.max(s.abs()));
    amplitude_to_db(peak)
}

/// Есть ли в записи хоть одно окно громче порога.
///
/// Порог задаётся в дБFS (в конфиге `audio.silence_threshold_db`, по умолчанию −45).
pub fn is_silent(audio: &PcmAudio, threshold_db: f32) -> bool {
    loudest_window_db(audio) < threshold_db
}

/// Обрезать тишину по краям, оставив `keep_ms` запаса до первого и после последнего звука.
///
/// Возвращает пустой буфер той же частоты, если громче порога не оказалось ни одного окна.
/// Внутренние паузы не трогаются.
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
        .map(|(idx, _)| idx)
        .collect();

    let (Some(&first), Some(&last)) = (voiced.first(), voiced.last()) else {
        return PcmAudio::new(Vec::new(), audio.sample_rate);
    };

    let keep = (audio.sample_rate as u64 * keep_ms as u64 / 1000) as usize;
    let start = (first * window).saturating_sub(keep);
    let end = ((last + 1) * window + keep).min(audio.samples.len());

    PcmAudio::new(audio.samples[start..end].to_vec(), audio.sample_rate)
}

/// Обрезка по настройкам пользователя.
///
/// При `audio.trim_silence = false` запись возвращается как есть. Внутренние паузы не режутся
/// никогда, поэтому `audio.vad_min_pause_ms` соблюдается по построению: пауза любой длины внутри
/// реплики остаётся на месте (E-01/02).
pub fn trim_for_config(audio: &PcmAudio, cfg: &AudioConfig) -> PcmAudio {
    if !cfg.trim_silence {
        return audio.clone();
    }
    trim_silence(audio, cfg.silence_threshold_db, DEFAULT_KEEP_MS)
}

/// Уровень самого громкого окна; для пустого буфера — [`SILENCE_FLOOR_DB`].
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

/// Длина окна анализа в отсчётах; нулевая частота даёт 0 и трактуется вызывающим как «нет данных».
fn window_len(sample_rate: u32) -> usize {
    (sample_rate as u64 * WINDOW_MS as u64 / 1000) as usize
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Амплитуда 0.0..=1.0 → дБFS, с полом на [`SILENCE_FLOOR_DB`].
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

    /// Отрезок синуса заданной амплитуды: ровный сигнал, у которого RMS предсказуем.
    fn tone(ms: u32, amplitude: f32) -> Vec<f32> {
        let n = (RATE as u64 * ms as u64 / 1000) as usize;
        (0..n).map(|i| amplitude * (i as f32 * 0.3).sin()).collect()
    }

    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (RATE as u64 * ms as u64 / 1000) as usize]
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

        // Осталась речь ± одно окно анализа (20 мс).
        let expected = (RATE as f32 * 0.3) as usize;
        let slack = window_len(RATE) * 2;
        assert!(
            out.samples.len() <= expected + slack,
            "не обрезано: {} отсчётов при ожидаемых ~{expected}",
            out.samples.len()
        );
        assert!(out.samples.len() + slack >= expected, "срезана сама речь");
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
            "запас keep_ms не оставлен: {} отсчётов",
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

        // Пауза внутри реплики остаётся: длина почти не изменилась.
        assert!(
            out.samples.len() * 100 >= total * 95,
            "внутренняя пауза вырезана: {} из {total}",
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
            .expect("в тоне есть громкий отсчёт");
        samples.extend(speech.clone());
        let out = trim_silence(&audio(samples), -45.0, 20);

        // Начало речи не съедено: громкий отсчёт остался в первых 100 мс результата.
        let head = &out.samples[..out.samples.len().min(RATE as usize / 10)];
        assert!(
            head.iter().any(|s| s.abs() > 0.4),
            "начало речи потеряно (первый громкий отсчёт был на {first_loud})"
        );
    }

    #[test]
    fn quiet_noise_below_threshold_is_treated_as_silence() {
        let out = trim_silence(&audio(tone(1000, 0.001)), -45.0, 0);
        assert!(out.samples.is_empty(), "шум −60 дБ принят за речь");
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

        // Порог −45 дБ: речь громче, её оставляем.
        let normal = AudioConfig::default();
        assert!(!trim_for_config(&input, &normal).samples.is_empty());

        // Порог 0 дБ: громче нет ничего, значит тишина целиком.
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
