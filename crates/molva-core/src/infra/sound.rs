// SPDX-License-Identifier: MIT
//! Звуковые метки записи через cpal: короткий синус на устройство вывода по умолчанию.
//!
//! Реализация критериев «звуковой сигнал начала и конца записи» и «сигнал отключается
//! настройкой»: [`build_sound_cue`] по `audio.sounds` отдаёт либо [`CpalSoundCue`], либо
//! [`NullSoundCue`], который молчит. Громкость берётся из `audio.sound_volume`.
//!
//! Играть звук синхронно нельзя: демон зовёт `play` из управляющего потока прямо перед тем, как
//! открыть микрофон, и восемьдесят миллисекунд ожидания съели бы бюджет реакции на клавишу.
//! Поэтому каждый сигнал уходит в отдельную короткоживущую нить.

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use tracing::{debug, warn};

use crate::config::AudioConfig;
use crate::domain::sound::{CueKind, SoundCue};

/// Длина плавного нарастания и затухания: без них короткий тон щёлкает на границах.
const FADE_MS: f32 = 8.0;

/// Тишина: `audio.sounds = false`.
///
/// Отдельный тип, а не `Option<Arc<dyn SoundCue>>` у вызывающего: выключенный звук не должен
/// требовать ветвления в демоне, иначе одну из веток однажды забудут.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSoundCue;

impl SoundCue for NullSoundCue {
    fn id(&self) -> &'static str {
        "null"
    }

    fn play(&self, _kind: CueKind) {}
}

/// Сигналы синусом через устройство вывода по умолчанию.
#[derive(Debug, Clone, Copy)]
pub struct CpalSoundCue {
    volume: f32,
}

impl CpalSoundCue {
    /// `volume` — доля от полной шкалы, приводится к диапазону 0…1.
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
        // Звук не должен задерживать открытие микрофона: играем в стороне.
        let spawned = std::thread::Builder::new()
            .name("molva-sound".into())
            .spawn(move || {
                if let Err(err) = play_tone(kind, volume) {
                    debug!(cue = kind.label(), error = %err, "сигнал не проигран");
                }
            });
        if let Err(err) = spawned {
            warn!(error = %err, "не удалось запустить нить звукового сигнала");
        }
    }
}

/// Проигрыватель по настройкам: выключенный звук — это `NullSoundCue`, а не отключённый вызов.
pub fn build_sound_cue(audio: &AudioConfig) -> Arc<dyn SoundCue> {
    if !audio.sounds {
        return Arc::new(NullSoundCue);
    }
    Arc::new(CpalSoundCue::new(audio.sound_volume))
}

/// Один отсчёт синуса с плавными краями.
///
/// Чистая функция: форма сигнала проверяется тестами без звуковой карты.
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

/// Сколько отсчётов занимает сигнал на этой частоте дискретизации.
pub fn tone_len(kind: CueKind, sample_rate: u32) -> usize {
    (sample_rate as u64 * u64::from(kind.duration_ms()) / 1000) as usize
}

/// Открыть поток вывода, проиграть тон и закрыть поток.
fn play_tone(kind: CueKind, volume: f32) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "устройства вывода нет".to_string())?;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("конфигурация вывода недоступна: {e}"))?;
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
        other => Err(format!("формат вывода не поддержан: {other:?}")),
    }?;
    stream
        .play()
        .map_err(|e| format!("поток не запущен: {e}"))?;
    // Поток живёт ровно столько, сколько звучит тон, плюс запас на буфер устройства.
    std::thread::sleep(Duration::from_millis(u64::from(kind.duration_ms()) + 60));
    drop(stream);
    Ok(())
}

/// Генератор тона: помнит, сколько отсчётов уже отдал, и молчит после конца сигнала.
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

/// Построить поток вывода для конкретного типа отсчётов.
fn build_output<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    mut tone: Tone,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    // Каждый кадр — `channels` одинаковых отсчётов: тон моно, панорамировать нечего.
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
            move |err| warn!(error = %err, "ошибка потока вывода звука"),
            None,
        )
        .map_err(|e| format!("поток вывода не открыт: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_switches_the_cues_off() {
        // Критерий AG-06: `audio.sounds = false` — и звука нет вообще, а не «тихо».
        let mut audio = AudioConfig {
            sounds: false,
            ..AudioConfig::default()
        };
        assert_eq!(build_sound_cue(&audio).id(), "null");
        // Молчаливая реализация обязана быть безопасной без звуковой карты.
        build_sound_cue(&audio).play(CueKind::RecordStart);

        audio.sounds = true;
        assert_eq!(build_sound_cue(&audio).id(), "cpal");
    }

    #[test]
    fn zero_volume_plays_nothing() {
        let cue = CpalSoundCue::new(0.0);
        // Нулевая громкость не открывает устройство вывода: тест не трогает железо.
        cue.play(CueKind::RecordStop);
        assert_eq!(
            CpalSoundCue::new(2.0).volume,
            1.0,
            "громкость ограничена 1.0"
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
        assert!(peak > 0.3, "сигнал должен быть слышен: {peak}");
    }

    #[test]
    fn the_length_of_a_cue_matches_its_duration() {
        assert_eq!(tone_len(CueKind::RecordStart, 16_000), 16_000 * 80 / 1000);
        assert_eq!(tone_len(CueKind::Error, 16_000), 16_000 * 160 / 1000);
    }
}
