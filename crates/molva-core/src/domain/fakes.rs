// SPDX-License-Identifier: MIT
//! Фейки контрактов для тестов всех крейтов.
//!
//! Каждый фейк записывает, как его вызывали: тест ассертит наблюдаемый эффект, а не факт запуска.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use super::audio::{AudioError, AudioSource, PcmAudio};
use super::clock::Clock;
use super::entry::Entry;
use super::inject::{InjectError, InjectReport, OutputMode, TextInjector};
use super::journal::{Journal, JournalError};
use super::llm::{ChatRequest, ChatResponse, LlmClient, LlmError};
use super::notify::Notifier;
use super::sound::{CueKind, SoundCue};
use super::stt::{SttEngine, SttError, SttOptions, Transcript};

/// Источник, отдающий заранее заданный буфер.
///
/// В режиме порций (`paced`) он ведёт себя как микрофон: во время записи звук снимается частями,
/// а `stop` всё равно отдаёт запись целиком. Так проверяется потоковая обработка без железа.
#[derive(Debug, Clone)]
pub struct FakeAudioSource {
    audio: PcmAudio,
    recording: bool,
    pub start_calls: usize,
    /// Сколько отсчётов отдавать за один `drain_new_samples`; `None` — не отдавать вовсе.
    portion: Option<usize>,
    /// Позиция чтения потоковой обработки.
    drained: usize,
}

impl FakeAudioSource {
    pub fn from_pcm(audio: PcmAudio) -> Self {
        Self {
            audio,
            recording: false,
            start_calls: 0,
            portion: None,
            drained: 0,
        }
    }

    /// Секунда тишины на 16 кГц.
    pub fn silence(secs: f32) -> Self {
        let n = (secs * 16_000.0) as usize;
        Self::from_pcm(PcmAudio::new(vec![0.0; n], 16_000))
    }

    /// Тот же буфер, но во время записи он снимается порциями по `portion_ms`.
    pub fn paced(audio: PcmAudio, portion_ms: u32) -> Self {
        let portion = (u64::from(audio.sample_rate) * u64::from(portion_ms) / 1000).max(1) as usize;
        Self {
            portion: Some(portion),
            ..Self::from_pcm(audio)
        }
    }
}

impl AudioSource for FakeAudioSource {
    fn start(&mut self, level_tx: Option<Sender<f32>>) -> Result<(), AudioError> {
        if self.recording {
            return Err(AudioError::AlreadyRecording);
        }
        self.recording = true;
        self.start_calls += 1;
        self.drained = 0;
        if let Some(tx) = level_tx {
            // Ошибка отправки означает, что слушателя уже нет; для фейка это не сбой.
            let _ = tx.send(self.audio.rms());
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<PcmAudio, AudioError> {
        if !self.recording {
            return Err(AudioError::NotRecording);
        }
        self.recording = false;
        Ok(self.audio.clone())
    }

    fn is_recording(&self) -> bool {
        self.recording
    }

    fn drain_new_samples(&mut self) -> Option<PcmAudio> {
        let portion = self.portion?;
        if !self.recording || self.drained >= self.audio.samples.len() {
            return None;
        }
        let to = (self.drained + portion).min(self.audio.samples.len());
        let samples = self.audio.samples[self.drained..to].to_vec();
        self.drained = to;
        Some(PcmAudio::new(samples, self.audio.sample_rate))
    }
}

/// Распознаватель с очередью заранее заданных ответов; запоминает параметры каждого вызова.
#[derive(Debug)]
pub struct FakeStt {
    responses: VecDeque<Result<Transcript, SttError>>,
    pub calls: Vec<SttOptions>,
    pub unload_calls: usize,
}

impl FakeStt {
    /// Всегда возвращает один и тот же текст.
    pub fn returning(text: &str) -> Self {
        let mut fake = Self::with_responses(vec![]);
        fake.responses
            .push_back(Ok(Transcript::text_only(text.to_string())));
        fake
    }

    /// Ответы выдаются по очереди; когда очередь пуста, повторяется последний.
    pub fn with_responses(responses: Vec<Result<Transcript, SttError>>) -> Self {
        Self {
            responses: responses.into(),
            calls: Vec::new(),
            unload_calls: 0,
        }
    }
}

impl SttEngine for FakeStt {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn model_name(&self) -> &'static str {
        "fake"
    }

    fn transcribe(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
    ) -> Result<Transcript, SttError> {
        self.calls.push(options.clone());
        if audio.samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        match self.responses.len() {
            0 => Err(SttError::Inference("у фейка нет ответов".into())),
            1 => self.responses[0].clone(),
            _ => self
                .responses
                .pop_front()
                .unwrap_or(Err(SttError::EmptyAudio)),
        }
    }

    fn unload(&mut self) {
        self.unload_calls += 1;
    }
}

/// Обработчик запроса к фейковой модели.
type LlmHandler = Box<dyn Fn(&ChatRequest) -> Result<ChatResponse, LlmError> + Send + Sync>;

/// Модель с настраиваемым ответом и счётчиком вызовов.
pub struct FakeLlm {
    handler: LlmHandler,
    calls: AtomicUsize,
    pub last_request: Mutex<Option<ChatRequest>>,
}

// У замыкания-обработчика нет `Debug`, поэтому в отчёт идёт то, что тесту и нужно видеть.
impl std::fmt::Debug for FakeLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeLlm")
            .field("calls", &self.calls)
            .field("last_request", &self.last_request)
            .finish_non_exhaustive()
    }
}

impl FakeLlm {
    pub fn new(
        handler: impl Fn(&ChatRequest) -> Result<ChatResponse, LlmError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
            calls: AtomicUsize::new(0),
            last_request: Mutex::new(None),
        }
    }

    /// Всегда отвечает одним и тем же текстом.
    pub fn echoing(text: &str) -> Self {
        let text = text.to_string();
        Self::new(move |_| {
            Ok(ChatResponse {
                text: text.clone(),
                prompt_tokens: Some(10),
                completion_tokens: Some(5),
            })
        })
    }

    /// Всегда падает — для проверки fallback на сырой текст.
    pub fn failing(error: LlmError) -> Self {
        Self::new(move |_| Err(error.clone()))
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LlmClient for FakeLlm {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn is_local(&self) -> bool {
        true
    }

    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut last) = self.last_request.lock() {
            *last = Some(request.clone());
        }
        (self.handler)(request)
    }
}

/// Записывает, что просили вставить; может имитировать отказ.
#[derive(Debug, Default)]
pub struct RecordingInjector {
    pub injected: Vec<(String, OutputMode)>,
    pub fail_with: Option<InjectError>,
    pub selection: Option<String>,
}

impl RecordingInjector {
    pub fn with_selection(text: &str) -> Self {
        Self {
            selection: Some(text.to_string()),
            ..Self::default()
        }
    }
}

impl TextInjector for RecordingInjector {
    fn id(&self) -> &'static str {
        "recording"
    }

    fn available(&self) -> bool {
        true
    }

    fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        self.injected.push((text.to_string(), mode));
        Ok(InjectReport {
            method: format!("recording-{mode:?}").to_lowercase(),
            attempts: vec![],
        })
    }

    fn copy_selection(&mut self) -> Result<String, InjectError> {
        self.selection.clone().ok_or(InjectError::Unsupported)
    }
}

/// Часы, которыми управляет тест.
#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<DateTime<Utc>>,
    base: Instant,
    offset: Mutex<Duration>,
    /// Каждая пауза, о которой попросил код продукта: тест проверяет её, не ожидая по-настоящему.
    slept: Mutex<Vec<Duration>>,
}

impl FakeClock {
    pub fn at(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
            base: Instant::now(),
            offset: Mutex::new(Duration::ZERO),
            slept: Mutex::new(Vec::new()),
        }
    }

    /// Паузы в порядке запроса.
    pub fn slept(&self) -> Vec<Duration> {
        self.slept.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn advance(&self, by: Duration) {
        if let Ok(mut now) = self.now.lock() {
            *now += chrono::Duration::from_std(by).unwrap_or_default();
        }
        if let Ok(mut offset) = self.offset.lock() {
            *offset += by;
        }
    }
}

impl Clock for FakeClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now.lock().map_or_else(|_| Utc::now(), |n| *n)
    }

    fn instant(&self) -> Instant {
        let offset = self.offset.lock().map(|o| *o).unwrap_or_default();
        self.base + offset
    }

    /// Пауза в тесте не ждёт: она записывается и двигает часы вперёд.
    fn sleep(&self, duration: Duration) {
        if let Ok(mut slept) = self.slept.lock() {
            slept.push(duration);
        }
        self.advance(duration);
    }
}

/// Собирает уведомления в память.
#[derive(Debug, Default)]
pub struct RecordingNotifier {
    pub messages: Mutex<Vec<(String, String)>>,
}

impl Notifier for RecordingNotifier {
    fn notify(&self, title: &str, body: &str) {
        if let Ok(mut messages) = self.messages.lock() {
            messages.push((title.to_string(), body.to_string()));
        }
    }
}

/// Запоминает сыгранные сигналы: тест считает, сколько их было на реплику.
#[derive(Debug, Default)]
pub struct RecordingSoundCue {
    played: Mutex<Vec<CueKind>>,
}

impl RecordingSoundCue {
    /// Сигналы в порядке воспроизведения.
    pub fn played(&self) -> Vec<CueKind> {
        self.played.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

impl SoundCue for RecordingSoundCue {
    fn id(&self) -> &'static str {
        "recording"
    }

    fn play(&self, kind: CueKind) {
        if let Ok(mut played) = self.played.lock() {
            played.push(kind);
        }
    }
}

/// Журнал в памяти.
#[derive(Debug, Default)]
pub struct MemJournal {
    pub entries: Vec<Entry>,
}

impl Journal for MemJournal {
    fn append(&mut self, entry: &Entry) -> Result<(), JournalError> {
        self.entries.push(entry.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::stt::LanguageHint;

    #[test]
    fn fake_audio_source_requires_start_before_stop() {
        let mut source = FakeAudioSource::silence(0.1);
        assert_eq!(source.stop(), Err(AudioError::NotRecording));
        source.start(None).unwrap();
        assert!(source.is_recording());
        assert_eq!(source.start(None), Err(AudioError::AlreadyRecording));
        let audio = source.stop().unwrap();
        assert_eq!(audio.samples.len(), 1600);
    }

    #[test]
    fn a_paced_source_hands_out_portions_and_still_returns_everything_at_stop() {
        let audio = PcmAudio::new((0..1_600).map(|i| i as f32).collect(), 16_000);
        let mut source = FakeAudioSource::paced(audio.clone(), 50);
        source.start(None).unwrap();

        let first = source.drain_new_samples().expect("первая порция");
        let second = source.drain_new_samples().expect("вторая порция");

        assert_eq!(first.samples.len(), 800, "50 мс при 16 кГц — 800 отсчётов");
        assert_eq!(
            second.samples[0], 800.0,
            "порции идут подряд, без пропусков"
        );
        assert!(
            source.drain_new_samples().is_none(),
            "буфер кончился, отдавать нечего"
        );
        assert_eq!(
            source.stop().unwrap(),
            audio,
            "stop обязан вернуть запись целиком, а не хвост после снятых порций"
        );
    }

    #[test]
    fn a_plain_source_reports_that_it_cannot_stream() {
        let mut source = FakeAudioSource::silence(1.0);
        source.start(None).unwrap();
        assert!(source.drain_new_samples().is_none());
    }

    #[test]
    fn fake_stt_records_options_and_replays_last_answer() {
        let mut stt = FakeStt::returning("привет");
        let audio = PcmAudio::new(vec![0.1; 16_000], 16_000);
        let options = SttOptions {
            language: LanguageHint::Fixed("ru".into()),
            ..SttOptions::default()
        };
        assert_eq!(stt.transcribe(&audio, &options).unwrap().text, "привет");
        assert_eq!(stt.transcribe(&audio, &options).unwrap().text, "привет");
        assert_eq!(stt.calls.len(), 2);
        assert_eq!(stt.calls[0].language, LanguageHint::Fixed("ru".into()));
    }

    #[test]
    fn fake_stt_rejects_empty_audio() {
        let mut stt = FakeStt::returning("x");
        let error = stt
            .transcribe(&PcmAudio::default(), &SttOptions::default())
            .unwrap_err();
        assert_eq!(error, SttError::EmptyAudio);
    }

    #[test]
    fn fake_llm_counts_calls_and_keeps_last_request() {
        let llm = FakeLlm::echoing("ok");
        let request = ChatRequest {
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            temperature: 0.0,
            max_tokens: 10,
        };
        assert_eq!(llm.complete(&request).unwrap().text, "ok");
        assert_eq!(llm.calls(), 1);
        assert_eq!(llm.last_request.lock().unwrap().as_ref().unwrap().user, "u");
        let failing = FakeLlm::failing(LlmError::Timeout(20));
        assert_eq!(failing.complete(&request), Err(LlmError::Timeout(20)));
    }

    #[test]
    fn recording_injector_captures_text_and_can_fail() {
        let mut injector = RecordingInjector::default();
        injector.inject("текст", OutputMode::Type).unwrap();
        assert_eq!(
            injector.injected,
            vec![("текст".to_string(), OutputMode::Type)]
        );
        injector.fail_with = Some(InjectError::Failed("нет окна".into()));
        assert!(injector.inject("ещё", OutputMode::Paste).is_err());
        assert_eq!(injector.injected.len(), 1);
    }

    #[test]
    fn fake_clock_advances_both_wall_and_monotonic_time() {
        let start = DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let clock = FakeClock::at(start);
        let t0 = clock.instant();
        clock.advance(Duration::from_secs(90));
        assert_eq!(clock.now_utc() - start, chrono::Duration::seconds(90));
        assert_eq!(clock.instant() - t0, Duration::from_secs(90));
    }
}
