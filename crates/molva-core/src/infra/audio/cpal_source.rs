// SPDX-License-Identifier: MIT
//! Захват с микрофона через cpal.
//!
//! Поток открывается в [`CpalSource::start`] и закрывается в [`CpalSource::stop`]: вне записи
//! микрофон свободен, индикатор в системе гаснет, приватность соблюдена (AG-01/03).
//!
//! `cpal::Stream` не `Send`, а `AudioSource` обязан быть `Send`, поэтому потоком владеет
//! отдельный нить-хозяин: она строит поток, играет его и закрывает по команде `stop`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use tracing::{debug, info, warn};

use crate::config::AudioConfig;
use crate::domain::audio::{downmix_to_mono, AudioError, AudioSource, DeviceInfo, PcmAudio};
use crate::infra::audio::level::ZeroLevelWatch;

/// Имя устройства, означающее «системное по умолчанию».
pub const DEFAULT_DEVICE: &str = "default";

/// Частоты, которые показываем пользователю, если устройство сообщает диапазон.
const COMMON_RATES: [u32; 5] = [16_000, 22_050, 32_000, 44_100, 48_000];

/// Не чаще 10 раз в секунду: индикатор уровня в GUI большего не требует, а канал не забивается.
const LEVEL_INTERVAL: Duration = Duration::from_millis(100);

/// Список устройств ввода: имя, признак «по умолчанию», поддерживаемые частоты.
///
/// Список пересоставляется при каждом вызове, поэтому подключённый на ходу микрофон виден без
/// перезапуска демона (hot-plug).
pub fn list_input_devices() -> Result<Vec<DeviceInfo>, AudioError> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().map(|d| d.to_string());

    let devices = host
        .input_devices()
        .map_err(|e| map_cpal_error(&e, DEFAULT_DEVICE))?;

    let mut out: Vec<DeviceInfo> = Vec::new();
    for device in devices {
        let name = device.to_string();
        let sample_rates = match device.supported_input_configs() {
            Ok(configs) => sample_rates_from_ranges(
                configs.map(|c| (c.min_sample_rate(), c.max_sample_rate())),
            ),
            Err(e) => {
                debug!(device = %name, error = %e, "не удалось прочитать конфигурации устройства");
                Vec::new()
            }
        };
        merge_device(
            &mut out,
            DeviceInfo {
                is_default: default_name.as_deref() == Some(name.as_str()),
                name,
                sample_rates,
            },
        );
    }

    // ALSA отдаёт устройство по умолчанию отдельной записью, которой нет в общем списке:
    // без неё пользователь не увидит, куда на самом деле идёт запись.
    if let Some(default_name) = default_name {
        if !out.iter().any(|d| d.name == default_name) {
            out.insert(
                0,
                DeviceInfo {
                    name: default_name,
                    is_default: true,
                    sample_rates: Vec::new(),
                },
            );
        }
    }

    if out.is_empty() {
        return Err(AudioError::NoDevices);
    }
    Ok(out)
}

/// Добавить устройство, объединив частоты с уже найденным одноимённым.
///
/// ALSA показывает одно физическое устройство несколько раз (hw, plughw, разные форматы):
/// пользователю нужен один пункт списка, иначе выбирать не из чего.
fn merge_device(devices: &mut Vec<DeviceInfo>, incoming: DeviceInfo) {
    if let Some(existing) = devices.iter_mut().find(|d| d.name == incoming.name) {
        existing.is_default |= incoming.is_default;
        existing.sample_rates.extend(incoming.sample_rates);
        existing.sample_rates.sort_unstable();
        existing.sample_rates.dedup();
        return;
    }
    devices.push(incoming);
}

/// Микрофон как источник записи.
pub struct CpalSource {
    device: String,
    gain: f32,
    max_duration_secs: u32,
    active: Option<Active>,
    /// Упёрлась ли в лимит последняя завершённая запись; читается уже после `stop`.
    truncated: bool,
}

/// Состояние идущей записи: нить-хозяин потока и общий буфер.
struct Active {
    stop_tx: Sender<()>,
    thread: JoinHandle<()>,
    shared: Arc<Shared>,
    sample_rate: u32,
    /// Сколько отсчётов уже забрала потоковая обработка: позиция чтения, а не удаление.
    drained: usize,
}

/// То, что callback пишет, а `stop` читает.
struct Shared {
    samples: Mutex<Vec<f32>>,
    /// Сколько отсчётов моно помещается в `max_duration_secs`.
    max_samples: usize,
    /// Запись упёрлась в лимит длительности.
    truncated: AtomicBool,
    /// Поток сообщил об ошибке: устройство отключили или конфигурация протухла.
    lost: AtomicBool,
}

impl CpalSource {
    /// `device` — имя из [`list_input_devices`] или `"default"`.
    pub fn new(device: &str, gain: f32, max_duration_secs: u32) -> Self {
        Self {
            device: device.to_string(),
            gain,
            max_duration_secs,
            active: None,
            truncated: false,
        }
    }

    /// Источник по настройкам пользователя.
    pub fn from_config(cfg: &AudioConfig) -> Self {
        Self::new(&cfg.device, cfg.gain, cfg.max_duration_secs)
    }

    /// Упёрлась ли запись в `max_duration_secs` — во время записи и после её остановки.
    ///
    /// Демону это нужно уже после `stop`, чтобы сказать пользователю, что хвост не сохранён.
    pub fn was_truncated(&self) -> bool {
        match &self.active {
            Some(active) => active.shared.truncated.load(Ordering::Relaxed),
            None => self.truncated,
        }
    }
}

impl AudioSource for CpalSource {
    fn start(&mut self, level_tx: Option<Sender<f32>>) -> Result<(), AudioError> {
        if self.active.is_some() {
            return Err(AudioError::AlreadyRecording);
        }
        self.truncated = false;

        // Список устройств пересоставляется на каждый старт: микрофон могли переподключить.
        let devices = list_input_devices()?;
        let selected = pick_device_name(&self.device, &devices)?;

        let (ready_tx, ready_rx) = channel::<Result<(u32, Arc<Shared>), AudioError>>();
        let (stop_tx, stop_rx) = channel::<()>();
        let max_duration_secs = self.max_duration_secs;
        let requested = self.device.clone();

        let thread = std::thread::Builder::new()
            .name("molva-audio".into())
            .spawn(move || {
                run_stream(
                    selected,
                    requested,
                    max_duration_secs,
                    level_tx,
                    ready_tx,
                    stop_rx,
                )
            })
            .map_err(|e| AudioError::Backend(format!("не удалось запустить поток записи: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok((sample_rate, shared))) => {
                info!(device = %self.device, sample_rate, "запись начата");
                self.active = Some(Active {
                    stop_tx,
                    thread,
                    shared,
                    sample_rate,
                    drained: 0,
                });
                Ok(())
            }
            Ok(Err(err)) => {
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                let _ = thread.join();
                Err(AudioError::Backend(
                    "нить захвата завершилась, не открыв поток".into(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<PcmAudio, AudioError> {
        let Some(active) = self.active.take() else {
            return Err(AudioError::NotRecording);
        };
        // Ошибка отправки означает, что нить уже завершилась сама (например, потеряв устройство).
        let _ = active.stop_tx.send(());
        if active.thread.join().is_err() {
            return Err(AudioError::Backend(
                "нить захвата аварийно завершилась".into(),
            ));
        }

        let lost = active.shared.lost.load(Ordering::Relaxed);
        let truncated = active.shared.truncated.load(Ordering::Relaxed);
        self.truncated = truncated;
        let mut samples = match active.shared.samples.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };

        if samples.is_empty() && lost {
            return Err(AudioError::DeviceLost);
        }
        if lost {
            warn!(
                secs = samples.len() as f32 / active.sample_rate as f32,
                "устройство пропало во время записи, сохранено то, что успели записать"
            );
        }
        if truncated {
            warn!(
                max_duration_secs = self.max_duration_secs,
                "достигнут лимит длительности записи, хвост не сохранён"
            );
        }

        apply_gain(&mut samples, self.gain);
        info!(
            secs = samples.len() as f32 / active.sample_rate as f32,
            sample_rate = active.sample_rate,
            "запись остановлена"
        );
        // Частота остаётся родной: приведение к 16 кГц делает вызывающий через `to_16k()`,
        // чтобы сохранить исходник для отладки и не ресемплить дважды.
        Ok(PcmAudio::new(samples, active.sample_rate))
    }

    fn is_recording(&self) -> bool {
        self.active.is_some()
    }

    fn drain_new_samples(&mut self) -> Option<PcmAudio> {
        let gain = self.gain;
        let active = self.active.as_mut()?;
        let buffer = active.shared.samples.lock().ok()?;
        if buffer.len() <= active.drained {
            return None;
        }
        // Копия, а не изъятие: `stop` обязан вернуть запись целиком, иначе пропадёт и файл, и
        // длительность реплики. Усиление применяется к копии, поэтому дважды оно не наложится.
        let mut samples = buffer[active.drained..].to_vec();
        active.drained = buffer.len();
        drop(buffer);
        apply_gain(&mut samples, gain);
        Some(PcmAudio::new(samples, active.sample_rate))
    }
}

impl Drop for CpalSource {
    fn drop(&mut self) {
        // Микрофон не должен остаться открытым, если источник уронили без stop().
        if let Some(active) = self.active.take() {
            let _ = active.stop_tx.send(());
            let _ = active.thread.join();
        }
    }
}

/// Тело нити-хозяина потока: строит поток, отвечает на `ready`, ждёт команды остановки.
fn run_stream(
    selected: Option<String>,
    requested: String,
    max_duration_secs: u32,
    level_tx: Option<Sender<f32>>,
    ready_tx: Sender<Result<(u32, Arc<Shared>), AudioError>>,
    stop_rx: Receiver<()>,
) {
    match open_stream(selected, &requested, max_duration_secs, level_tx) {
        Ok((stream, sample_rate, shared)) => {
            if ready_tx.send(Ok((sample_rate, shared))).is_err() {
                return;
            }
            // Ошибка recv означает, что владелец исчез: поток всё равно пора закрывать.
            let _ = stop_rx.recv();
            drop(stream);
        }
        Err(err) => {
            let _ = ready_tx.send(Err(err));
        }
    }
}

/// Открыть поток на выбранном устройстве и начать накопление отсчётов.
fn open_stream(
    selected: Option<String>,
    requested: &str,
    max_duration_secs: u32,
    level_tx: Option<Sender<f32>>,
) -> Result<(cpal::Stream, u32, Arc<Shared>), AudioError> {
    let host = cpal::default_host();
    let device = match &selected {
        // Устройство по умолчанию может не встречаться в общем списке (так делает ALSA), поэтому
        // имя сверяется и с ним: иначе выбранный в GUI пункт списка не открылся бы.
        Some(name) => host
            .input_devices()
            .map_err(|e| map_cpal_error(&e, requested))?
            .chain(host.default_input_device())
            .find(|d| d.to_string() == *name)
            .ok_or_else(|| AudioError::DeviceNotFound(name.clone()))?,
        None => host.default_input_device().ok_or(AudioError::NoDevices)?,
    };

    let supported = device
        .default_input_config()
        .map_err(|e| map_cpal_error(&e, requested))?;
    let sample_format = supported.sample_format();
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let config: StreamConfig = supported.config();

    let shared = Arc::new(Shared {
        samples: Mutex::new(Vec::with_capacity(sample_rate as usize)),
        max_samples: sample_rate as usize * max_duration_secs.max(1) as usize,
        truncated: AtomicBool::new(false),
        lost: AtomicBool::new(false),
    });

    debug!(
        device = %device.to_string(),
        %sample_rate,
        channels,
        format = ?sample_format,
        "открываю поток записи"
    );

    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, channels, &shared, level_tx),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, channels, &shared, level_tx),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, channels, &shared, level_tx),
        SampleFormat::I8 => build_stream::<i8>(&device, &config, channels, &shared, level_tx),
        SampleFormat::U8 => build_stream::<u8>(&device, &config, channels, &shared, level_tx),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, channels, &shared, level_tx),
        SampleFormat::F64 => build_stream::<f64>(&device, &config, channels, &shared, level_tx),
        other => Err(AudioError::Backend(format!(
            "устройство отдаёт неподдерживаемый формат отсчётов: {other:?}"
        ))),
    }?;

    stream.play().map_err(|e| map_cpal_error(&e, requested))?;
    Ok((stream, sample_rate, shared))
}

/// Построить поток для конкретного типа отсчётов; всё сводится к моно `f32`.
fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    shared: &Arc<Shared>,
    level_tx: Option<Sender<f32>>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let data_shared = Arc::clone(shared);
    let error_shared = Arc::clone(shared);
    let mut last_level = Instant::now() - LEVEL_INTERVAL;
    let mut zero_level = ZeroLevelWatch::with_defaults();

    device
        .build_input_stream::<T, _, _>(
            *config,
            move |data: &[T], _| {
                let floats: Vec<f32> = data.iter().map(|s| f32::from_sample(*s)).collect();
                let mono = downmix_to_mono(&floats, channels);

                let now = Instant::now();
                if now.duration_since(last_level) >= LEVEL_INTERVAL {
                    last_level = now;
                    let level = rms(&mono);
                    if let Some(message) = zero_level.observe(level, now) {
                        warn!("{message}");
                    }
                    if let Some(tx) = &level_tx {
                        // Слушателя может уже не быть — для записи это не сбой.
                        let _ = tx.send(level);
                    }
                }

                match data_shared.samples.lock() {
                    Ok(mut buffer) => {
                        let room = data_shared.max_samples.saturating_sub(buffer.len());
                        if room == 0 {
                            data_shared.truncated.store(true, Ordering::Relaxed);
                            return;
                        }
                        if mono.len() > room {
                            data_shared.truncated.store(true, Ordering::Relaxed);
                        }
                        buffer.extend_from_slice(&mono[..mono.len().min(room)]);
                    }
                    Err(_) => data_shared.lost.store(true, Ordering::Relaxed),
                }
            },
            move |err| {
                warn!(error = %err, "ошибка потока записи");
                error_shared.lost.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| map_cpal_error(&e, &device.to_string()))
}

/// Какое устройство открывать: `None` — системное по умолчанию.
///
/// Имя сверяется без учёта регистра; если такого нет, ошибка перечисляет доступные, чтобы
/// пользователю не пришлось отдельно звать `molva devices`.
pub fn pick_device_name(
    requested: &str,
    available: &[DeviceInfo],
) -> Result<Option<String>, AudioError> {
    let requested = requested.trim();
    if available.is_empty() {
        return Err(AudioError::NoDevices);
    }
    if requested.is_empty() || requested.eq_ignore_ascii_case(DEFAULT_DEVICE) {
        return Ok(None);
    }
    if let Some(found) = available
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(requested))
    {
        return Ok(Some(found.name.clone()));
    }
    let names: Vec<&str> = available.iter().map(|d| d.name.as_str()).collect();
    Err(AudioError::DeviceNotFound(format!(
        "{requested}. Доступны: {}",
        names.join(", ")
    )))
}

/// Усиление входа с ограничением: перегруженный сигнал ломает распознавание сильнее тихого.
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if !gain.is_finite() || (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    let gain = gain.max(0.0);
    for sample in samples.iter_mut() {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

/// Похожа ли частота на реальный режим устройства.
///
/// ALSA сообщает границы «любой частоты» как 1 Гц и 4294967295 Гц: показывать их пользователю
/// бессмысленно, выбрать их нельзя.
fn is_plausible_rate(rate: &u32) -> bool {
    (4_000..=768_000).contains(rate)
}

/// Частоты для показа пользователю: границы диапазонов плюс общеупотребимые значения внутри них.
fn sample_rates_from_ranges(ranges: impl Iterator<Item = (u32, u32)>) -> Vec<u32> {
    let mut rates = Vec::new();
    for (min, max) in ranges {
        rates.extend([min, max].into_iter().filter(is_plausible_rate));
        rates.extend(
            COMMON_RATES
                .iter()
                .copied()
                .filter(|r| *r > min && *r < max),
        );
    }
    rates.sort_unstable();
    rates.dedup();
    rates
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Ошибка cpal в термины домена: пользователю нужен следующий шаг, а не код бэкенда.
fn map_cpal_error(err: &cpal::Error, device: &str) -> AudioError {
    match err.kind() {
        cpal::ErrorKind::PermissionDenied => AudioError::PermissionDenied(format!(
            "{device}: {err}. Проверьте права доступа к микрофону"
        )),
        cpal::ErrorKind::DeviceNotAvailable => AudioError::DeviceNotFound(device.to_string()),
        cpal::ErrorKind::DeviceBusy => {
            AudioError::Backend(format!("{device} занято другим приложением: {err}"))
        }
        cpal::ErrorKind::HostUnavailable => AudioError::Backend(format!(
            "аудиоподсистема недоступна: {err}. Проверьте, что PipeWire или ALSA запущены"
        )),
        cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
            AudioError::Backend(format!("{device} не поддерживает запись: {err}"))
        }
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::StreamInvalidated => {
            AudioError::DeviceLost
        }
        _ => AudioError::Backend(format!("{device}: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices() -> Vec<DeviceInfo> {
        vec![
            DeviceInfo {
                name: "alsa_input.pci-0000_00_1f.3.analog-stereo".into(),
                is_default: true,
                sample_rates: vec![48_000],
            },
            DeviceInfo {
                name: "Yeti Stereo Microphone".into(),
                is_default: false,
                sample_rates: vec![16_000, 48_000],
            },
        ]
    }

    #[test]
    fn default_keyword_means_system_default() {
        assert_eq!(pick_device_name("default", &devices()).expect("ok"), None);
        assert_eq!(pick_device_name("  ", &devices()).expect("ok"), None);
    }

    #[test]
    fn device_is_matched_by_name_ignoring_case() {
        assert_eq!(
            pick_device_name("yeti stereo microphone", &devices()).expect("ok"),
            Some("Yeti Stereo Microphone".into())
        );
    }

    #[test]
    fn unknown_device_error_lists_available_ones() {
        let err = pick_device_name("Blue Snowball", &devices()).expect_err("такого нет");
        let AudioError::DeviceNotFound(message) = err else {
            panic!("ожидалась DeviceNotFound, получено {err:?}");
        };
        assert!(message.contains("Blue Snowball"), "нет имени запрошенного");
        assert!(
            message.contains("Yeti Stereo Microphone"),
            "перечень доступных устройств не подсказан: {message}"
        );
    }

    #[test]
    fn empty_device_list_is_reported_as_no_devices() {
        assert_eq!(
            pick_device_name("default", &[]).expect_err("нет устройств"),
            AudioError::NoDevices
        );
    }

    #[test]
    fn gain_scales_and_clamps() {
        let mut samples = vec![0.25, -0.25, 0.9];
        apply_gain(&mut samples, 2.0);
        assert_eq!(samples, vec![0.5, -0.5, 1.0], "пик обязан быть ограничен");
    }

    #[test]
    fn gain_of_one_leaves_samples_untouched() {
        let mut samples = vec![0.1, -0.2];
        apply_gain(&mut samples, 1.0);
        assert_eq!(samples, vec![0.1, -0.2]);
    }

    #[test]
    fn broken_gain_value_does_not_zero_the_recording() {
        let mut samples = vec![0.1, -0.2];
        apply_gain(&mut samples, f32::NAN);
        assert_eq!(samples, vec![0.1, -0.2]);

        let mut samples = vec![0.1, -0.2];
        apply_gain(&mut samples, -1.0);
        assert_eq!(
            samples,
            vec![0.0, 0.0],
            "отрицательное усиление трактуется как 0"
        );
    }

    #[test]
    fn sample_rates_include_bounds_and_common_values() {
        let rates = sample_rates_from_ranges([(8_000, 48_000)].into_iter());
        assert_eq!(rates, vec![8_000, 16_000, 22_050, 32_000, 44_100, 48_000]);
    }

    #[test]
    fn absurd_alsa_bounds_are_not_shown_as_sample_rates() {
        // ALSA отдаёт «любую частоту» как 1 … 4294967295: в списке остаются только реальные.
        let rates = sample_rates_from_ranges([(1, u32::MAX)].into_iter());
        assert_eq!(rates, COMMON_RATES.to_vec());
    }

    #[test]
    fn duplicate_alsa_entries_collapse_into_one_device() {
        let mut devices = Vec::new();
        merge_device(
            &mut devices,
            DeviceInfo {
                name: "ALC897 Analog".into(),
                is_default: false,
                sample_rates: vec![44_100],
            },
        );
        merge_device(
            &mut devices,
            DeviceInfo {
                name: "ALC897 Analog".into(),
                is_default: true,
                sample_rates: vec![16_000, 44_100],
            },
        );

        assert_eq!(devices.len(), 1, "одно устройство — один пункт списка");
        assert_eq!(devices[0].sample_rates, vec![16_000, 44_100]);
        assert!(devices[0].is_default, "признак «по умолчанию» потерян");
    }

    #[test]
    fn sample_rates_are_deduplicated_across_ranges() {
        let rates = sample_rates_from_ranges([(44_100, 44_100), (48_000, 48_000)].into_iter());
        assert_eq!(rates, vec![44_100, 48_000]);
    }

    #[test]
    fn permission_error_keeps_the_hint() {
        let err = map_cpal_error(&cpal::Error::new(cpal::ErrorKind::PermissionDenied), "Yeti");
        let AudioError::PermissionDenied(message) = err else {
            panic!("ожидалась PermissionDenied, получено {err:?}");
        };
        assert!(message.contains("Yeti"));
    }

    #[test]
    fn missing_device_maps_to_domain_error() {
        assert_eq!(
            map_cpal_error(
                &cpal::Error::new(cpal::ErrorKind::DeviceNotAvailable),
                "Yeti"
            ),
            AudioError::DeviceNotFound("Yeti".into())
        );
    }

    #[test]
    fn stream_invalidation_is_a_lost_device() {
        assert_eq!(
            map_cpal_error(
                &cpal::Error::new(cpal::ErrorKind::StreamInvalidated),
                "Yeti"
            ),
            AudioError::DeviceLost
        );
    }

    #[test]
    fn source_takes_device_gain_and_limit_from_the_config() {
        let cfg = AudioConfig {
            device: "Yeti".into(),
            gain: 1.5,
            max_duration_secs: 42,
            ..AudioConfig::default()
        };

        let source = CpalSource::from_config(&cfg);

        assert_eq!(source.device, "Yeti");
        assert_eq!(source.gain, 1.5);
        assert_eq!(source.max_duration_secs, 42);
    }

    #[test]
    fn stop_without_start_is_an_error() {
        let mut source = CpalSource::new(DEFAULT_DEVICE, 1.0, 600);
        assert!(!source.is_recording());
        assert_eq!(
            source.stop().expect_err("записи не было"),
            AudioError::NotRecording
        );
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0.0; 16]), 0.0);
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5, -0.5]) - 0.5).abs() < 1e-6);
    }
}
