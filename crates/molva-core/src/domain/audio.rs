// SPDX-License-Identifier: MIT
//! Аудио: PCM-буфер, источник захвата и чистые преобразования.
//!
//! Распознаватель принимает моно 16 кГц `f32`, поэтому всё, что приходит с микрофона или из
//! файла, приводится к этому виду здесь, независимо от бэкенда.

use std::sync::mpsc::Sender;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Частота, которую ждёт whisper.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;

/// Моно-сигнал с известной частотой дискретизации.
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

    /// Длительность в секундах; для пустого буфера или нулевой частоты — 0.
    pub fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }

    /// Среднеквадратичный уровень сигнала, 0.0 для пустого буфера.
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f32 = self.samples.iter().map(|s| s * s).sum();
        (sum / self.samples.len() as f32).sqrt()
    }

    /// Приведение к 16 кГц; если частота уже целевая — копия без изменений.
    pub fn to_16k(&self) -> PcmAudio {
        resample_linear(self, TARGET_SAMPLE_RATE)
    }
}

/// Сведение чередующихся каналов в моно усреднением.
///
/// `channels == 0` трактуется как моно, чтобы битые заголовки не роняли конвейер.
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

/// Линейная интерполяция между соседними отсчётами.
///
/// Для речи под whisper этого достаточно; качественный полифазный ресемплер (rubato) можно
/// подключить позже за той же сигнатурой.
pub fn resample_linear(audio: &PcmAudio, target_rate: u32) -> PcmAudio {
    if audio.sample_rate == target_rate || audio.samples.is_empty() || audio.sample_rate == 0 {
        return PcmAudio::new(audio.samples.clone(), target_rate);
    }
    let ratio = audio.sample_rate as f64 / target_rate as f64;
    let out_len = ((audio.samples.len() as f64) / ratio).round().max(1.0) as usize;
    let last = audio.samples.len() - 1;
    let samples = (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = (pos.floor() as usize).min(last);
            let next = (idx + 1).min(last);
            let frac = (pos - idx as f64) as f32;
            audio.samples[idx] * (1.0 - frac) + audio.samples[next] * frac
        })
        .collect();
    PcmAudio::new(samples, target_rate)
}

/// Устройство ввода, как его видит пользователь в CLI и GUI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub sample_rates: Vec<u32>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AudioError {
    #[error("устройство ввода не найдено: {0}")]
    DeviceNotFound(String),
    #[error("устройства ввода отсутствуют")]
    NoDevices,
    #[error("устройство отключено во время записи")]
    DeviceLost,
    #[error("доступ к микрофону запрещён: {0}")]
    PermissionDenied(String),
    #[error("запись не запущена")]
    NotRecording,
    #[error("запись уже идёт")]
    AlreadyRecording,
    #[error("не удалось декодировать {path}: {reason}")]
    Decode { path: String, reason: String },
    #[error("ошибка аудиоподсистемы: {0}")]
    Backend(String),
}

/// Источник аудио: микрофон в проде, WAV-фикстура в тестах.
///
/// Приватность микрофона — гарантия контракта, а не свойство конкретной реализации:
///
/// - **вне записи микрофон выключен**: поток открывается только в `start` и закрывается в `stop`,
///   между репликами MolvAI не слушает ничего и индикатор записи в системе погашен;
/// - **включается по клавише быстро**: `start` открывает поток синхронно и возвращает управление,
///   когда захват уже идёт — от нажатия до записи меньше 200 мс;
/// - **освобождается сразу после реплики**: `stop` закрывает поток и отдаёт записанное, а не
///   оставляет его открытым «на всякий случай» — от отпускания клавиши до закрытия меньше 500 мс.
///
/// Реализация обязана освобождать устройство и при уничтожении: реплика могла закончиться
/// аварийно, но микрофон после этого всё равно свободен.
pub trait AudioSource: Send {
    /// Начать захват. `level_tx` получает RMS-уровень не чаще ~10 раз в секунду.
    fn start(&mut self, level_tx: Option<Sender<f32>>) -> Result<(), AudioError>;
    /// Остановить захват, освободить микрофон и вернуть всё записанное.
    fn stop(&mut self) -> Result<PcmAudio, AudioError>;
    /// Идёт ли захват прямо сейчас. `false` означает, что микрофон свободен.
    fn is_recording(&self) -> bool;
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
        // 2 → 4 отсчёта: середина между 0.0 и 1.0 должна стать 0.5
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
