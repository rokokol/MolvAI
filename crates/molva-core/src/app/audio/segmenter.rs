// SPDX-License-Identifier: MIT
//! Нарезка ещё идущей записи на куски для распознавания.
//!
//! Реплику незачем распознавать целиком после отпускания клавиши: пока человек говорит, процессор
//! простаивает. Сегментатор накапливает поток и отдаёт готовый кусок, как только набралось
//! достаточно звука и человек сделал паузу — такой кусок уходит в whisper прямо во время речи, а
//! после отпускания остаётся распознать только хвост.
//!
//! Резать посреди слога нельзя: whisper по обрывку слова уверенно печатает не то слово. Отсюда два
//! правила. Граница ищется по середине паузы, а не по таймеру; когда пауз нет вовсе и кусок дорос
//! до предела, граница ставится по самому тихому окну последних двух секунд. Соседние куски
//! перекрываются на [`SegmenterConfig::overlap_ms`], чтобы звук на самой границе попал в оба.
//!
//! Структура чистая: ни времени, ни потоков, ни ввода-вывода — только отсчёты на входе и куски на
//! выходе, поэтому проверяется синтетическими сигналами без микрофона и без модели.

use crate::domain::audio::PcmAudio;

/// Куска короче этого не бывает: на паузу до него сегментатор не реагирует.
pub const DEFAULT_TARGET_CHUNK_SECS: f32 = 5.0;

/// Предел, после которого кусок режется даже без паузы.
pub const DEFAULT_MAX_CHUNK_SECS: f32 = 12.0;

/// Перекрытие соседних кусков: звук на границе попадает в оба куска.
pub const DEFAULT_OVERLAP_MS: u32 = 150;

/// Пауза, по которой режется кусок, если про неё не сказано в настройках.
pub const DEFAULT_CHUNK_PAUSE_MS: u32 = 700;

/// Окно анализа уровня: как в [`crate::app::audio::trim`], 20 мс.
const WINDOW_MS: u32 = 20;

/// Хвост, в котором ищется самое тихое окно, когда кусок дорос до предела.
const QUIETEST_LOOKBACK_MS: u32 = 2_000;

/// Уровень, который считаем абсолютной тишиной: логарифм нуля не определён.
const SILENCE_FLOOR_DB: f32 = -120.0;

/// Настройки нарезки.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmenterConfig {
    /// Частота дискретизации потока; куски отдаются на ней же.
    pub sample_rate: u32,
    /// Раньше этого кусок не отдаётся, даже если пауза уже была.
    pub target_chunk_secs: f32,
    /// Позже этого кусок режется по самому тихому окну, даже если паузы не было.
    pub max_chunk_secs: f32,
    /// Пауза короче этой границей не считается.
    pub min_pause_ms: u32,
    /// Порог тишины в дБFS, как `audio.silence_threshold_db`.
    pub silence_threshold_db: f32,
    /// Перекрытие соседних кусков.
    pub overlap_ms: u32,
}

impl SegmenterConfig {
    /// Настройки по умолчанию для частоты потока: пауза и порог приходят из конфига аудио.
    pub fn new(sample_rate: u32, min_pause_ms: u32, silence_threshold_db: f32) -> Self {
        Self {
            sample_rate,
            target_chunk_secs: DEFAULT_TARGET_CHUNK_SECS,
            max_chunk_secs: DEFAULT_MAX_CHUNK_SECS,
            min_pause_ms,
            silence_threshold_db,
            overlap_ms: DEFAULT_OVERLAP_MS,
        }
    }
}

/// Готовый кусок записи.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub audio: PcmAudio,
    /// Порядковый номер куска в реплике, начиная с нуля.
    pub index: usize,
    /// Смещение начала куска от начала записи.
    pub start_ms: u32,
}

impl Chunk {
    pub fn duration_secs(&self) -> f32 {
        self.audio.duration_secs()
    }
}

/// Нарезчик потока на куски.
pub struct Segmenter {
    config: SegmenterConfig,
    /// Ещё не отданные отсчёты; начинается с перекрытия предыдущего куска.
    buffer: Vec<f32>,
    /// Уровень каждого целого окна буфера в дБFS.
    windows: Vec<f32>,
    /// Абсолютный индекс `buffer[0]` от начала записи: из него считается `start_ms`.
    offset: usize,
    /// Сколько кусков уже отдано.
    emitted: usize,
}

impl Segmenter {
    pub fn new(config: SegmenterConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            windows: Vec::new(),
            offset: 0,
            emitted: 0,
        }
    }

    pub fn config(&self) -> &SegmenterConfig {
        &self.config
    }

    /// Сколько кусков отдано с начала записи.
    pub fn emitted(&self) -> usize {
        self.emitted
    }

    /// Добавить свежие отсчёты и забрать всё, что дозрело до куска.
    ///
    /// Один вызов может отдать несколько кусков: если поток пришёл большой порцией, границ в нём
    /// может оказаться сразу несколько.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Chunk> {
        self.buffer.extend_from_slice(samples);
        let mut out = Vec::new();
        if self.config.sample_rate == 0 {
            // Без частоты длительности нет: резать не по чему, всё уйдёт хвостом.
            return out;
        }
        loop {
            self.analyse();
            if self.buffer.len() < self.target_samples() {
                break;
            }
            // Пауза важнее предела: если она есть, резать по ней всегда лучше.
            let cut = self
                .pause_cut()
                .or_else(|| (self.buffer.len() >= self.max_samples()).then(|| self.quietest_cut()));
            let Some(cut) = cut else { break };
            if let Some(chunk) = self.take(cut) {
                out.push(chunk);
            }
        }
        out
    }

    /// Хвост записи: всё, что осталось в буфере и не было отдано.
    ///
    /// `None`, если в остатке нет ни одного окна громче порога: распознавать тишину незачем.
    pub fn finish(&mut self) -> Option<Chunk> {
        self.analyse();
        let cut = self.buffer.len();
        if cut == 0 {
            return None;
        }
        self.take(cut)
    }

    /// Длина окна анализа в отсчётах; для нулевой частоты — один отсчёт.
    fn window_len(&self) -> usize {
        ((self.config.sample_rate as u64 * WINDOW_MS as u64 / 1000) as usize).max(1)
    }

    fn samples_for_secs(&self, secs: f32) -> usize {
        (self.config.sample_rate as f32 * secs.max(0.0)) as usize
    }

    fn samples_for_ms(&self, ms: u32) -> usize {
        (self.config.sample_rate as u64 * ms as u64 / 1000) as usize
    }

    fn target_samples(&self) -> usize {
        self.samples_for_secs(self.config.target_chunk_secs).max(1)
    }

    fn max_samples(&self) -> usize {
        self.samples_for_secs(self.config.max_chunk_secs)
            .max(self.target_samples())
    }

    /// Посчитать уровень для целых окон, которые появились после прошлого вызова.
    fn analyse(&mut self) {
        let window = self.window_len();
        while (self.windows.len() + 1) * window <= self.buffer.len() {
            let from = self.windows.len() * window;
            let level = amplitude_to_db(rms(&self.buffer[from..from + window]));
            self.windows.push(level);
        }
    }

    fn is_silent_window(&self, index: usize) -> bool {
        self.windows[index] < self.config.silence_threshold_db
    }

    /// Граница по середине первой достаточно длинной паузы после `target`.
    ///
    /// Пауза в конце буфера считается наравне с законченной: ждать, пока человек снова заговорит,
    /// значит потерять то самое время, ради которого всё и затевалось.
    fn pause_cut(&self) -> Option<usize> {
        let window = self.window_len();
        let min_run = (self.config.min_pause_ms as usize / WINDOW_MS as usize).max(1);
        let target = self.target_samples();

        let mut run_start: Option<usize> = None;
        for index in 0..=self.windows.len() {
            let silent = index < self.windows.len() && self.is_silent_window(index);
            if silent {
                run_start.get_or_insert(index);
                continue;
            }
            let Some(start) = run_start.take() else {
                continue;
            };
            if index - start < min_run {
                continue;
            }
            let cut = (start + index) / 2 * window;
            if cut >= target {
                return Some(cut.min(self.buffer.len()));
            }
        }
        None
    }

    /// Граница по самому тихому окну последних секунд: паузы не было, а резать пора.
    fn quietest_cut(&self) -> usize {
        let window = self.window_len();
        let fallback = self.max_samples().clamp(1, self.buffer.len());
        let limit = (self.max_samples() / window).min(self.windows.len());
        let lookback = (QUIETEST_LOOKBACK_MS as usize / WINDOW_MS as usize).max(1);
        let from = limit.saturating_sub(lookback).max(1);
        if from >= limit {
            return fallback;
        }
        let quietest = (from..limit).min_by(|a, b| {
            self.windows[*a]
                .partial_cmp(&self.windows[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // Середина самого тихого окна: слог не рвётся ни в одну, ни в другую сторону.
        quietest.map_or(fallback, |index| {
            (index * window + window / 2).min(self.buffer.len())
        })
    }

    /// Отрезать кусок по границе `cut` и оставить перекрытие для следующего.
    ///
    /// `None` означает, что в куске не было ни одного окна громче порога: такой кусок в whisper не
    /// уходит, иначе модель напечатает на тишине хвост субтитров.
    fn take(&mut self, cut: usize) -> Option<Chunk> {
        let window = self.window_len();
        let cut = cut.clamp(1, self.buffer.len());
        let voiced = self
            .windows
            .iter()
            .take(cut / window)
            .any(|level| *level >= self.config.silence_threshold_db);

        let samples = self.buffer[..cut].to_vec();
        let start_ms = self.ms_of(self.offset);

        // Перекрытие оставляем следующему куску; `max(1)` гарантирует движение вперёд.
        let keep_from = cut
            .saturating_sub(self.samples_for_ms(self.config.overlap_ms))
            .max(1)
            .min(cut);
        self.buffer.drain(..keep_from);
        self.offset += keep_from;
        // Окна считались от прежнего начала буфера: выравнивание сбилось, считаем заново.
        self.windows.clear();
        self.analyse();

        if !voiced {
            return None;
        }
        let index = self.emitted;
        self.emitted += 1;
        Some(Chunk {
            audio: PcmAudio::new(samples, self.config.sample_rate),
            index,
            start_ms,
        })
    }

    fn ms_of(&self, samples: usize) -> u32 {
        if self.config.sample_rate == 0 {
            return 0;
        }
        u32::try_from(samples as u64 * 1000 / self.config.sample_rate as u64).unwrap_or(u32::MAX)
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

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

    /// Короткие пороги: тест не должен генерировать по двенадцать секунд сигнала на проверку.
    fn config() -> SegmenterConfig {
        SegmenterConfig {
            sample_rate: RATE,
            target_chunk_secs: 0.5,
            max_chunk_secs: 1.5,
            min_pause_ms: 200,
            silence_threshold_db: -45.0,
            overlap_ms: 50,
        }
    }

    fn tone(ms: u32) -> Vec<f32> {
        tone_at(ms, 0.5)
    }

    fn tone_at(ms: u32, amplitude: f32) -> Vec<f32> {
        let n = (RATE as u64 * ms as u64 / 1000) as usize;
        (0..n).map(|i| amplitude * (i as f32 * 0.3).sin()).collect()
    }

    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (RATE as u64 * ms as u64 / 1000) as usize]
    }

    fn ms_of(chunk: &Chunk) -> u32 {
        (chunk.audio.samples.len() as u64 * 1000 / RATE as u64) as u32
    }

    #[test]
    fn a_pause_between_two_tones_becomes_the_boundary() {
        let mut segmenter = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(chunks.len(), 1, "пауза не стала границей: {chunks:?}");
        // Граница — середина паузы: 600 мс тона плюс половина от 400 мс тишины.
        let cut = ms_of(&chunks[0]);
        assert!(
            (750..=850).contains(&cut),
            "резать надо по середине паузы, а вышло на {cut} мс"
        );
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].start_ms, 0);
    }

    #[test]
    fn a_continuous_tone_is_cut_at_the_limit() {
        let mut segmenter = Segmenter::new(config());

        let chunks = segmenter.push(&tone(1_700));

        assert_eq!(chunks.len(), 1, "непрерывный тон не разрезан по пределу");
        let cut = ms_of(&chunks[0]);
        assert!(
            cut <= 1_500,
            "кусок должен уложиться в предел 1500 мс, а вышел на {cut} мс"
        );
        assert!(cut > 0, "кусок пустой");
    }

    #[test]
    fn without_a_pause_the_boundary_lands_on_the_quietest_window() {
        let mut segmenter = Segmenter::new(config());
        // Речь без пауз, но с провалом громкости на 1000 мс: резать надо там.
        let mut signal = tone(1_000);
        signal.extend(tone_at(100, 0.02));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(chunks.len(), 1);
        let cut = ms_of(&chunks[0]);
        assert!(
            (980..=1_120).contains(&cut),
            "граница должна попасть в провал на 1000 мс, а вышла на {cut} мс"
        );
    }

    #[test]
    fn a_short_recording_gives_a_single_chunk_at_the_end() {
        let mut segmenter = Segmenter::new(config());

        assert!(
            segmenter.push(&tone(300)).is_empty(),
            "кусок короче target отдавать рано"
        );
        let tail = segmenter.finish().expect("хвост есть");

        assert_eq!(tail.index, 0);
        assert_eq!(ms_of(&tail), 300);
        assert_eq!(segmenter.emitted(), 1);
    }

    #[test]
    fn an_empty_recording_yields_nothing() {
        let mut segmenter = Segmenter::new(config());
        assert!(segmenter.push(&[]).is_empty());
        assert!(segmenter.finish().is_none());
        assert_eq!(segmenter.emitted(), 0);
    }

    #[test]
    fn silence_alone_never_reaches_the_model() {
        let mut segmenter = Segmenter::new(config());
        // Две секунды тишины перевалили и за target, и за предел.
        assert!(
            segmenter.push(&silence(2_000)).is_empty(),
            "тишина ушла бы в whisper и вернулась «Продолжением следует»"
        );
        assert!(segmenter.finish().is_none());
    }

    #[test]
    fn neighbouring_chunks_overlap_so_a_syllable_is_not_cut_in_half() {
        let mut config = config();
        config.overlap_ms = 100;
        let mut segmenter = Segmenter::new(config);

        // Три тона через паузы: две границы, значит два куска и хвост.
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));
        signal.extend(silence(400));
        signal.extend(tone(600));
        let chunks = segmenter.push(&signal);
        let tail = segmenter.finish().expect("хвост есть");

        assert_eq!(chunks.len(), 2, "ожидались две границы: {chunks:?}");
        let first_end = chunks[0].start_ms + ms_of(&chunks[0]);
        assert!(
            chunks[1].start_ms + 100 >= first_end && chunks[1].start_ms < first_end,
            "куски не перекрылись: первый кончается на {first_end}, второй начинается на {}",
            chunks[1].start_ms
        );
        assert_eq!(tail.index, 2, "нумерация кусков сквозная");
    }

    #[test]
    fn a_pause_before_the_target_is_not_a_boundary() {
        let mut segmenter = Segmenter::new(config());
        // Пауза на 200 мс — раньше target в 500 мс: кусок был бы слишком коротким.
        let mut signal = tone(150);
        signal.extend(silence(300));
        signal.extend(tone(200));

        assert!(segmenter.push(&signal).is_empty());
        assert_eq!(segmenter.emitted(), 0);
    }

    #[test]
    fn a_stream_arriving_in_small_portions_is_cut_the_same_way() {
        let mut whole = Segmenter::new(config());
        let mut piecemeal = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));

        let at_once = whole.push(&signal);
        let mut streamed = Vec::new();
        // Порции по 100 мс — так демон снимает звук с микрофона во время записи.
        for portion in signal.chunks(RATE as usize / 10) {
            streamed.extend(piecemeal.push(portion));
        }

        assert_eq!(at_once.len(), 1);
        assert_eq!(streamed.len(), 1, "поток порциями не дал куска");
        // Обе границы попали в паузу 600…1000 мс. Потоком режется раньше: как только пауза
        // дотянула до min_pause, ждать её конца незачем.
        for cut in [ms_of(&at_once[0]), ms_of(&streamed[0])] {
            assert!((600..=1_000).contains(&cut), "граница вне паузы: {cut} мс");
        }
        assert!(
            ms_of(&streamed[0]) <= ms_of(&at_once[0]),
            "потоковая нарезка не должна ждать дольше пакетной"
        );
    }

    #[test]
    fn start_ms_counts_from_the_beginning_of_the_recording() {
        let mut segmenter = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));
        signal.extend(silence(400));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(chunks[0].start_ms, 0);
        assert!(
            chunks[1].start_ms >= 700,
            "второй кусок начинается после первого: {}",
            chunks[1].start_ms
        );
    }

    #[test]
    fn defaults_match_the_documented_lengths() {
        let config = SegmenterConfig::new(16_000, DEFAULT_CHUNK_PAUSE_MS, -45.0);
        assert_eq!(config.target_chunk_secs, 5.0);
        assert_eq!(config.max_chunk_secs, 12.0);
        assert_eq!(config.overlap_ms, 150);
        assert_eq!(config.min_pause_ms, 700);
    }

    #[test]
    fn a_zero_sample_rate_does_not_divide_by_zero() {
        let mut segmenter = Segmenter::new(SegmenterConfig::new(0, 700, -45.0));
        assert!(segmenter.push(&[0.5; 100]).is_empty());
        let tail = segmenter.finish().expect("хвост отдан как есть");
        assert_eq!(tail.start_ms, 0);
    }
}
