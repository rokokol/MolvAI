// SPDX-License-Identifier: MIT
//! Звуковые метки записи: короткий сигнал в начале и в конце реплики.
//!
//! Критерий: пользователь слышит **сигнал начала записи и сигнал конца записи** — ровно два
//! звука на одну реплику, и ни одного лишнего. Звук — единственная обратная связь, когда окно
//! MolvAI не видно, поэтому он привязан к переходам машины состояний, а не к событиям ввода.
//!
//! Второй критерий: **сигналы отключаются настройкой** `audio.sounds = false`. Отключение живёт
//! не в вызывающем коде, а в реализации: демон всегда зовёт [`SoundCue::play`], а тишину делает
//! `NullSoundCue` из `infra::sound`. Так «выключено» нельзя забыть проверить в одной из веток.

/// Какой именно сигнал играть.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CueKind {
    /// Запись началась: микрофон открыт, можно говорить.
    RecordStart,
    /// Запись закончилась: микрофон закрыт, реплика ушла в обработку.
    RecordStop,
    /// Что-то не получилось: микрофон не открылся, запись отброшена.
    Error,
}

impl CueKind {
    /// Частота тона в герцах: старт выше конца, ошибка заметно ниже обоих.
    pub fn frequency_hz(self) -> f32 {
        match self {
            CueKind::RecordStart => 880.0,
            CueKind::RecordStop => 660.0,
            CueKind::Error => 220.0,
        }
    }

    /// Длительность сигнала в миллисекундах: короче щелчка мыши, чтобы не мешать речи.
    pub fn duration_ms(self) -> u32 {
        match self {
            CueKind::RecordStart | CueKind::RecordStop => 80,
            CueKind::Error => 160,
        }
    }

    /// Имя для логов.
    pub fn label(self) -> &'static str {
        match self {
            CueKind::RecordStart => "начало записи",
            CueKind::RecordStop => "конец записи",
            CueKind::Error => "ошибка",
        }
    }
}

/// Проигрыватель звуковых меток.
///
/// Реализация обязана возвращать управление сразу: демон зовёт `play` из управляющего потока,
/// и ожидание звука задержало бы открытие микрофона.
pub trait SoundCue: Send + Sync {
    /// Имя реализации: по нему видно, звучит ли что-нибудь вообще (`null` — выключено настройкой).
    fn id(&self) -> &'static str;
    fn play(&self, kind: CueKind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_and_stop_sound_different() {
        assert_ne!(
            CueKind::RecordStart.frequency_hz(),
            CueKind::RecordStop.frequency_hz(),
            "начало и конец записи должны различаться на слух"
        );
        assert!(CueKind::Error.frequency_hz() < CueKind::RecordStop.frequency_hz());
    }

    #[test]
    fn a_cue_is_short_enough_not_to_interrupt_speech() {
        for kind in [CueKind::RecordStart, CueKind::RecordStop, CueKind::Error] {
            assert!(kind.duration_ms() <= 200, "{kind:?}");
        }
    }
}
