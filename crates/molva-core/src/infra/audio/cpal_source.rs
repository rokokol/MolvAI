// SPDX-License-Identifier: MIT
//! Microphone capture through cpal
//!
//! The stream opens in [`CpalSource::start`] and closes in [`CpalSource::stop`]: outside a
//! recording the microphone is free, the system indicator goes off, and privacy is kept (AG-01/03)
//!
//! `cpal::Stream` is not `Send`, but `AudioSource` must be `Send`, so a separate owner thread
//! owns the stream: it builds the stream, plays it and closes it on the `stop` command

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

/// Device name that means "the system default"
pub const DEFAULT_DEVICE: &str = "default";

/// Sample rates that we show to the user if the device reports a range
const COMMON_RATES: [u32; 5] = [16_000, 22_050, 32_000, 44_100, 48_000];

/// At most 10 times per second: the level indicator in the GUI needs no more, and the channel does not clog
const LEVEL_INTERVAL: Duration = Duration::from_millis(100);

/// List of input devices: the name, the "default" flag, the supported sample rates
///
/// The list is rebuilt on each call, so a microphone connected on the fly is visible without
/// a restart of the daemon (hot-plug)
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
                debug!(device = %name, error = %e, "could not read the device configurations");
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

    // ALSA returns the default device as a separate entry that is not in the general list:
    // without it the user cannot see where the recording really goes
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

/// Add a device, merging the sample rates with an already found device of the same name
///
/// ALSA shows one physical device several times (hw, plughw, different formats): the user needs
/// one list item, otherwise there is nothing to choose from
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

/// Microphone as a recording source
#[derive(Debug)]
pub struct CpalSource {
    device: String,
    gain: f32,
    max_duration_secs: u32,
    active: Option<Active>,
    /// Whether the last finished recording hit the limit; read after `stop`
    truncated: bool,
    /// What a released recording captured: the microphone is released before its audio is collected
    released: Option<Result<PcmAudio, AudioError>>,
}

/// State of a running recording: the owner thread of the stream and the shared buffer
#[derive(Debug)]
struct Active {
    stop_tx: Sender<()>,
    thread: JoinHandle<()>,
    shared: Arc<Shared>,
    sample_rate: u32,
    /// How many samples the streaming processing has already taken: a read position, not a removal
    drained: usize,
}

/// What the callback writes and `stop` reads
#[derive(Debug)]
struct Shared {
    samples: Mutex<Vec<f32>>,
    /// How many mono samples fit in `max_duration_secs`
    max_samples: usize,
    /// The recording hit the duration limit
    truncated: AtomicBool,
    /// The stream reported an error: the device was disconnected or the configuration went stale
    lost: AtomicBool,
}

impl CpalSource {
    /// `device` is a name from [`list_input_devices`] or `"default"`
    pub fn new(device: &str, gain: f32, max_duration_secs: u32) -> Self {
        Self {
            device: device.to_string(),
            gain,
            max_duration_secs,
            active: None,
            truncated: false,
            released: None,
        }
    }

    /// Source from the user settings
    pub fn from_config(cfg: &AudioConfig) -> Self {
        Self::new(&cfg.device, cfg.gain, cfg.max_duration_secs)
    }

    /// Whether the recording hit `max_duration_secs` — during the recording and after it stops
    ///
    /// The daemon needs this after `stop`, to tell the user that the tail was not saved
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

        // The device list is rebuilt on each start: the microphone may have been reconnected.
        // But only when a specific name is chosen: a scan that polls each device takes up to
        // 300 ms (USB cards answer "busy"), and `default` does not need it —
        // this time was taken from the start of the phrase and turned a short tap into a key hold
        let requested = self.device.trim();
        let selected = if requested.is_empty() || requested.eq_ignore_ascii_case(DEFAULT_DEVICE) {
            None
        } else {
            let devices = list_input_devices()?;
            pick_device_name(&self.device, &devices)?
        };

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
                );
            })
            .map_err(|e| AudioError::Backend(format!("could not start the recording thread: {e}")))?;

        match ready_rx.recv() {
            Ok(Ok((sample_rate, shared))) => {
                info!(device = %self.device, sample_rate, "recording started");
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
                    "the capture thread ended without opening a stream".into(),
                ))
            }
        }
    }

    fn stop(&mut self) -> Result<PcmAudio, AudioError> {
        if !self.release_microphone() {
            return Err(AudioError::NotRecording);
        }
        self.released
            .take()
            .unwrap_or(Err(AudioError::NotRecording))
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
        // A copy, not a removal: `stop` must return the whole recording, or both the file and the
        // duration of the phrase are lost. The gain applies to the copy, so it cannot apply twice
        let mut samples = buffer[active.drained..].to_vec();
        active.drained = buffer.len();
        drop(buffer);
        apply_gain(&mut samples, gain);
        Some(PcmAudio::new(samples, active.sample_rate))
    }
}

impl CpalSource {
    /// Close the capture stream and release the microphone
    ///
    /// Guarantee AG-01/AG-03: between phrases the device is free and the system recording
    /// indicator is off. The method is called from [`AudioSource::stop`] and from `Drop`, so an
    /// abnormal end of a phrase does not leave the microphone open. The recorded audio goes to
    /// `released` and `stop` takes it, so the release of the device does not depend on whether
    /// anyone collected the audio
    ///
    /// Returns `false` if there was no recording
    pub fn release_microphone(&mut self) -> bool {
        let Some(active) = self.active.take() else {
            return false;
        };
        // A send error means the thread already ended by itself (for example, after losing the device)
        let _ = active.stop_tx.send(());
        let joined = active.thread.join().is_ok();
        info!(device = %self.device, "microphone released");
        self.released = Some(if joined {
            self.collect(&active.shared, active.sample_rate)
        } else {
            Err(AudioError::Backend(
                "the capture thread ended abnormally".into(),
            ))
        });
        true
    }

    /// Take the accumulated samples of a finished recording
    fn collect(&mut self, shared: &Arc<Shared>, sample_rate: u32) -> Result<PcmAudio, AudioError> {
        let lost = shared.lost.load(Ordering::Relaxed);
        let truncated = shared.truncated.load(Ordering::Relaxed);
        self.truncated = truncated;
        let mut samples = match shared.samples.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        };

        if samples.is_empty() && lost {
            return Err(AudioError::DeviceLost);
        }
        if lost {
            warn!(
                secs = samples.len() as f32 / sample_rate as f32,
                "device lost during the recording, what was already recorded is saved"
            );
        }
        if truncated {
            warn!(
                max_duration_secs = self.max_duration_secs,
                "recording duration limit reached, the tail is not saved"
            );
        }

        apply_gain(&mut samples, self.gain);
        info!(
            secs = samples.len() as f32 / sample_rate as f32,
            sample_rate, "recording stopped"
        );
        // The sample rate stays native: the caller converts to 16 kHz with `to_16k()`,
        // to keep the original for debugging and not to resample twice
        Ok(PcmAudio::new(samples, sample_rate))
    }
}

impl Drop for CpalSource {
    fn drop(&mut self) {
        // The microphone must not stay open if the source was dropped without stop()
        self.release_microphone();
    }
}

/// Body of the owner thread of the stream: it builds the stream, answers on `ready` and waits for the stop command
///
/// The channel ends and the device name arrive by value on purpose: the thread must own them,
/// or they do not close with it and the owner does not learn that the recording ended
#[allow(clippy::needless_pass_by_value)]
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
            // A recv error means the owner is gone: it is time to close the stream anyway
            let _ = stop_rx.recv();
            drop(stream);
        }
        Err(err) => {
            let _ = ready_tx.send(Err(err));
        }
    }
}

/// Open a stream on the chosen device and start to accumulate samples
// `level_tx` goes into the cpal callback and lives there: ownership is required
#[allow(clippy::needless_pass_by_value)]
fn open_stream(
    selected: Option<String>,
    requested: &str,
    max_duration_secs: u32,
    level_tx: Option<Sender<f32>>,
) -> Result<(cpal::Stream, u32, Arc<Shared>), AudioError> {
    let host = cpal::default_host();
    let device = match &selected {
        // The default device may be absent from the general list (ALSA does this), so the name
        // is compared with it too: otherwise the list item chosen in the GUI would not open
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
        "opening the recording stream"
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
            "the device returns an unsupported sample format: {other:?}"
        ))),
    }?;

    stream.play().map_err(|e| map_cpal_error(&e, requested))?;
    Ok((stream, sample_rate, shared))
}

/// Build a stream for a given sample type; everything is converted to mono `f32`
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
    // `None` means no level measurement yet: the very first data block returns a level at once
    let mut last_level: Option<Instant> = None;

    device
        .build_input_stream::<T, _, _>(
            *config,
            move |data: &[T], _| {
                let floats: Vec<f32> = data.iter().map(|s| f32::from_sample(*s)).collect();
                let mono = downmix_to_mono(&floats, channels);

                let now = Instant::now();
                if last_level.is_none_or(|last| now.duration_since(last) >= LEVEL_INTERVAL) {
                    last_level = Some(now);
                    let level = rms(&mono);
                    // The level listener notices a mute microphone, see ZeroLevelWatch
                    if let Some(tx) = &level_tx {
                        // The listener may be gone — for the recording this is not a failure
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
                warn!(error = %err, "recording stream error");
                error_shared.lost.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| map_cpal_error(&e, &device.to_string()))
}

/// Which device to open: `None` means the system default
///
/// The name is compared without case; if there is no such device, the error lists the available
/// ones, so the user does not need to call `molva devices` separately
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
        "{requested}. Available: {}",
        names.join(", ")
    )))
}

/// Input gain with a limit: an overloaded signal breaks recognition more than a quiet one
pub fn apply_gain(samples: &mut [f32], gain: f32) {
    if !gain.is_finite() || (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    let gain = gain.max(0.0);
    for sample in samples.iter_mut() {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}

/// Whether the sample rate looks like a real mode of the device
///
/// ALSA reports the bounds of "any sample rate" as 1 Hz and 4294967295 Hz: it makes no sense to
/// show them to the user, and the user cannot choose them
fn is_plausible_rate(rate: u32) -> bool {
    (4_000..=768_000).contains(&rate)
}

/// Sample rates to show to the user: the range bounds plus the common values inside them
fn sample_rates_from_ranges(ranges: impl Iterator<Item = (u32, u32)>) -> Vec<u32> {
    let mut rates = Vec::new();
    for (min, max) in ranges {
        rates.extend(
            [min, max]
                .into_iter()
                .filter(|rate| is_plausible_rate(*rate)),
        );
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

/// Convert a cpal error to domain terms: the user needs the next step, not a backend code
fn map_cpal_error(err: &cpal::Error, device: &str) -> AudioError {
    match err.kind() {
        cpal::ErrorKind::PermissionDenied => AudioError::PermissionDenied(format!(
            "{device}: {err}. Check the microphone access permissions"
        )),
        cpal::ErrorKind::DeviceNotAvailable => AudioError::DeviceNotFound(device.to_string()),
        cpal::ErrorKind::DeviceBusy => {
            AudioError::Backend(format!("{device} is in use by another application: {err}"))
        }
        cpal::ErrorKind::HostUnavailable => AudioError::Backend(format!(
            "audio subsystem is unavailable: {err}. Check that PipeWire or ALSA is running"
        )),
        cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
            AudioError::Backend(format!("{device} does not support recording: {err}"))
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
        let err = pick_device_name("Blue Snowball", &devices()).expect_err("no such device");
        let AudioError::DeviceNotFound(message) = err else {
            panic!("expected DeviceNotFound, got {err:?}");
        };
        assert!(message.contains("Blue Snowball"), "no name of the requested device");
        assert!(
            message.contains("Yeti Stereo Microphone"),
            "the list of available devices is not suggested: {message}"
        );
    }

    #[test]
    fn empty_device_list_is_reported_as_no_devices() {
        assert_eq!(
            pick_device_name("default", &[]).expect_err("no devices"),
            AudioError::NoDevices
        );
    }

    #[test]
    fn gain_scales_and_clamps() {
        let mut samples = vec![0.25, -0.25, 0.9];
        apply_gain(&mut samples, 2.0);
        assert_eq!(samples, vec![0.5, -0.5, 1.0], "the peak must be limited");
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
            "a negative gain counts as 0"
        );
    }

    #[test]
    fn sample_rates_include_bounds_and_common_values() {
        let rates = sample_rates_from_ranges([(8_000, 48_000)].into_iter());
        assert_eq!(rates, vec![8_000, 16_000, 22_050, 32_000, 44_100, 48_000]);
    }

    #[test]
    fn absurd_alsa_bounds_are_not_shown_as_sample_rates() {
        // ALSA returns "any sample rate" as 1 … 4294967295: only the real rates stay in the list
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

        assert_eq!(devices.len(), 1, "one device is one list item");
        assert_eq!(devices[0].sample_rates, vec![16_000, 44_100]);
        assert!(devices[0].is_default, "the \"default\" flag is lost");
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
            panic!("expected PermissionDenied, got {err:?}");
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
            source.stop().expect_err("there was no recording"),
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
