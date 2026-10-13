// SPDX-License-Identifier: MIT
//! Наблюдение за уровнем сигнала во время записи.
//!
//! Самая обидная потеря реплики — выключенный или замьюченный микрофон: запись идёт, индикатор
//! горит, а в буфере цифровой ноль. Здесь это замечается за секунду и сообщается один раз за
//! молчание, чтобы не превращать уведомления в шум.

use std::time::{Duration, Instant};

/// Ниже этого RMS сигнала фактически нет: −80 дБFS — это тишина цифрового нуля, а не тихая речь.
pub const ZERO_LEVEL_RMS: f32 = 1e-4;

/// Сколько ждать, прежде чем считать нулевой уровень проблемой, а не паузой перед словом.
pub const ZERO_LEVEL_WINDOW: Duration = Duration::from_secs(1);

/// Перерыв в замерах, после которого начинается новая запись. Внутри записи замеры идут
/// с шагом `LEVEL_INTERVAL` из `cpal_source`, а между записями их нет вовсе: тишина конца
/// прошлой записи не должна засчитываться следующей.
pub const RECORDING_GAP: Duration = Duration::from_secs(2);

/// Что показать пользователю при нулевом уровне.
pub const ZERO_LEVEL_MESSAGE: &str =
    "Микрофон не даёт сигнала. Проверьте, что он не выключен и выбран нужный вход \
     (`molva devices`)";

/// Следит за уровнем и говорит, когда пора предупредить о нулевом входе.
///
/// Время передаётся снаружи: тест не спит, а демон берёт его из своих часов.
#[derive(Debug)]
pub struct ZeroLevelWatch {
    threshold: f32,
    window: Duration,
    /// Когда начался текущий отрезок тишины.
    silent_since: Option<Instant>,
    /// Предупреждение за этот отрезок уже выдано.
    warned: bool,
    /// Время прошлого замера: перерыв дольше `RECORDING_GAP` — граница между записями.
    last_seen: Option<Instant>,
}

impl ZeroLevelWatch {
    pub fn new(threshold: f32, window: Duration) -> Self {
        Self {
            threshold,
            window,
            silent_since: None,
            warned: false,
            last_seen: None,
        }
    }

    /// Значения по умолчанию: −80 дБFS дольше секунды.
    pub fn with_defaults() -> Self {
        Self::new(ZERO_LEVEL_RMS, ZERO_LEVEL_WINDOW)
    }

    /// Учесть очередной замер уровня; вернуть текст предупреждения, если его пора показать.
    ///
    /// Предупреждение выдаётся один раз за непрерывный отрезок тишины: сигнал появился —
    /// счётчик сбрасывается, и следующее пропадание снова будет замечено.
    pub fn observe(&mut self, rms: f32, now: Instant) -> Option<&'static str> {
        if self
            .last_seen
            .is_some_and(|last| now.duration_since(last) >= RECORDING_GAP)
        {
            self.reset();
        }
        self.last_seen = Some(now);
        if rms > self.threshold {
            self.silent_since = None;
            self.warned = false;
            return None;
        }
        let since = *self.silent_since.get_or_insert(now);
        if self.warned || now.duration_since(since) < self.window {
            return None;
        }
        self.warned = true;
        Some(ZERO_LEVEL_MESSAGE)
    }

    /// Забыть накопленное: новая запись начинается с чистого листа.
    pub fn reset(&mut self) {
        self.silent_since = None;
        self.warned = false;
    }
}

impl Default for ZeroLevelWatch {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_silence_is_not_reported() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        assert_eq!(watch.observe(0.0, t0), None);
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(900)),
            None,
            "пауза перед первым словом не повод пугать пользователя"
        );
    }

    #[test]
    fn silence_longer_than_the_window_is_reported_once() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);

        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(1100)),
            Some(ZERO_LEVEL_MESSAGE)
        );
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(1200)),
            None,
            "повторные предупреждения об одной и той же тишине"
        );
    }

    #[test]
    fn a_pause_between_recordings_does_not_carry_silence_into_the_next_one() {
        // Демон наблюдает уровни всех записей одним наблюдателем: между записями замеров нет,
        // и тишина конца прошлой записи не должна засчитываться началу следующей.
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.observe(0.0, t0 + Duration::from_millis(500));

        let next = t0 + Duration::from_secs(10);
        assert_eq!(watch.observe(0.0, next), None);
        assert_eq!(
            watch.observe(0.0, next + Duration::from_millis(500)),
            None,
            "полсекунды тишины новой записи — ещё не повод предупреждать"
        );
        assert_eq!(
            watch.observe(0.0, next + Duration::from_millis(1100)),
            Some(ZERO_LEVEL_MESSAGE)
        );
    }

    #[test]
    fn signal_resets_the_watch() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.observe(0.0, t0 + Duration::from_millis(1100));

        watch.observe(0.2, t0 + Duration::from_millis(1200));

        assert_eq!(watch.observe(0.0, t0 + Duration::from_millis(1300)), None);
        assert_eq!(
            watch.observe(0.0, t0 + Duration::from_millis(2400)),
            Some(ZERO_LEVEL_MESSAGE),
            "пропавший во второй раз сигнал должен быть замечен снова"
        );
    }

    #[test]
    fn quiet_speech_is_not_zero_level() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        // −60 дБFS: тихо, но это сигнал, а не выключенный микрофон.
        watch.observe(0.001, t0);
        assert_eq!(watch.observe(0.001, t0 + Duration::from_secs(5)), None);
    }

    #[test]
    fn reset_forgets_the_silent_stretch() {
        let mut watch = ZeroLevelWatch::with_defaults();
        let t0 = Instant::now();
        watch.observe(0.0, t0);
        watch.reset();
        assert_eq!(watch.observe(0.0, t0 + Duration::from_millis(1100)), None);
    }
}
