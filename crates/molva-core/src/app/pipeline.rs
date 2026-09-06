// SPDX-License-Identifier: MIT
//! Конвейер одной реплики: аудио → распознавание → словарь → правила → модель → вставка → журнал.
//!
//! Главное свойство конвейера — реплику нельзя потерять. Модель не ответила, ключ протух, сервер
//! лёг: текст после правил всё равно доходит до вставки, в журнале стоит `llm_used = false`, а в
//! логе — предупреждение. Ошибка вставки тоже не роняет `run`: она попадает в `Entry.error`, а
//! текст остаётся в буфере обмена стараниями самой реализации вставки.
//!
//! Модель зовётся только когда сошлось всё сразу: стиль её требует, постобработка включена,
//! приватность разрешает и в реплике больше `rules.llm_min_words` слов. Короткие реплики
//! обрабатываются правилами — это и быстрее, и дешевле по токенам.

use std::sync::Arc;
use std::time::Instant;

use thiserror::Error;
use tracing::warn;
use uuid::Uuid;

use crate::config::{
    CommandModeConfig, Config, DictionaryConfig, LlmConfig, OutputConfig, PrivacyConfig,
    RulesConfig, SttConfig, StyleConfig,
};
use crate::domain::audio::PcmAudio;
use crate::domain::clock::Clock;
use crate::domain::entry::{Entry, LatencyMs, Mode, Source, Tokens, SCHEMA_VERSION};
use crate::domain::inject::{OutputMode, TextInjector};
use crate::domain::journal::{Journal, JournalError};
use crate::domain::llm::{ChatRequest, LlmClient, LlmError};
use crate::domain::stt::{LanguageHint, SttEngine, SttError, SttOptions, Transcript};
use crate::domain::text::word_count;

use super::daemon::chunked::{self, ChunkContext, ChunkPrefix, ChunkText};
use super::dictionary::Dictionary;
use super::llm_output::sanitize_llm_output;
use super::rules::RuleSet;
use super::styles::Styles;
use crate::infra::stt::{is_silence_hallucination, transcribe_with_language_policy};

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("распознавание не удалось: {0}")]
    Stt(#[from] SttError),
    #[error("журнал недоступен: {0}")]
    Journal(#[from] JournalError),
    #[error("не удалось получить выделенный текст: {0}")]
    Selection(String),
    #[error("режим команд выключен в настройках")]
    CommandModeDisabled,
    #[error("режим команд требует модели постобработки: {0}")]
    CommandModeNeedsLlm(LlmError),
}

/// Настройки, которые нужны конвейеру. Собираются из [`Config`] одним вызовом.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub stt: SttConfig,
    pub dictionary: DictionaryConfig,
    pub rules: RulesConfig,
    pub llm: LlmConfig,
    pub style: StyleConfig,
    pub output: OutputConfig,
    pub command_mode: CommandModeConfig,
    pub privacy: PrivacyConfig,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self::from_config(&Config::default())
    }
}

impl PipelineConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            stt: cfg.stt.clone(),
            dictionary: cfg.dictionary.clone(),
            rules: cfg.rules.clone(),
            llm: cfg.llm.clone(),
            style: cfg.style.clone(),
            output: cfg.output.clone(),
            command_mode: cfg.command_mode.clone(),
            privacy: cfg.privacy.clone(),
        }
    }
}

/// Разбор `output.mode` из настроек; неизвестное значение — `auto`.
fn output_mode(value: &str) -> OutputMode {
    match value.trim().to_lowercase().as_str() {
        "paste" => OutputMode::Paste,
        "type" => OutputMode::Type,
        "clipboard" => OutputMode::Clipboard,
        _ => OutputMode::Auto,
    }
}

/// Способ вставки, каким он попадёт в журнал.
///
/// Критерий AJ-09/AJ-10: если активного окна нет, вставлять некуда — текст остаётся в буфере
/// обмена, реализация вставки говорит об этом уведомлением, а в журнале случай виден отдельно
/// (`clipboard-no-focus`), а не выдаётся за удачную вставку в поле ввода.
pub const NO_FOCUS_METHOD: &str = "clipboard-no-focus";

fn inject_method_for(method: &str, app_hint: Option<&str>) -> String {
    if app_hint.is_none() && method == "clipboard-only" {
        return NO_FOCUS_METHOD.to_string();
    }
    method.to_string()
}

fn millis_since(start: Instant, now: Instant) -> u32 {
    now.saturating_duration_since(start)
        .as_millis()
        .min(u128::from(u32::MAX)) as u32
}

/// Конвейер одной реплики.
#[derive(Debug)]
pub struct Pipeline {
    stt: Box<dyn SttEngine>,
    llm: Option<Arc<dyn LlmClient>>,
    injector: Box<dyn TextInjector>,
    journal: Box<dyn Journal>,
    clock: Arc<dyn Clock>,
    config: PipelineConfig,
    dictionary: Dictionary,
    rules: RuleSet,
    styles: Styles,
    session_id: Uuid,
    /// От отпускания клавиши до закрытия потока микрофона: демон меряет, конвейер записывает.
    stop_after_release_ms: Option<u32>,
    /// Начало реплики, распознанное кусками во время записи; `None` — обычная реплика целиком.
    chunk_prefix: Option<ChunkPrefix>,
}

impl Pipeline {
    pub fn new(
        stt: Box<dyn SttEngine>,
        llm: Option<Arc<dyn LlmClient>>,
        injector: Box<dyn TextInjector>,
        journal: Box<dyn Journal>,
        clock: Arc<dyn Clock>,
        config: PipelineConfig,
    ) -> Self {
        let rules = RuleSet::from_config(&config.rules);
        let styles = Styles::from_config(&config.style);
        Self {
            stt,
            llm,
            injector,
            journal,
            clock,
            config,
            dictionary: Dictionary::empty(),
            rules,
            styles,
            session_id: Uuid::new_v4(),
            stop_after_release_ms: None,
            chunk_prefix: None,
        }
    }

    /// Записать замер демона: за сколько микрофон освободился после отпускания клавиши.
    ///
    /// Значение расходуется одной репликой: следующая реплика получит свой замер, а не чужой.
    pub fn set_stop_after_release(&mut self, ms: u32) {
        self.stop_after_release_ms = Some(ms);
    }

    /// Подключить словарь терминов.
    #[must_use]
    pub fn with_dictionary(mut self, dictionary: Dictionary) -> Self {
        self.dictionary = dictionary;
        self
    }

    pub fn set_dictionary(&mut self, dictionary: Dictionary) {
        self.dictionary = dictionary;
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dictionary
    }

    /// Перечитать словарь, если файл изменился.
    pub fn reload_dictionary(&mut self) -> bool {
        match self.dictionary.reload_if_changed() {
            Ok(changed) => changed,
            Err(err) => {
                warn!(error = %err, "словарь не перечитан, остаётся прежний");
                false
            }
        }
    }

    /// Применить новые настройки: правила и стили пересобираются.
    pub fn set_config(&mut self, config: PipelineConfig) {
        self.rules = RuleSet::from_config(&config.rules);
        self.styles = Styles::from_config(&config.style);
        self.config = config;
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    pub fn styles(&self) -> &Styles {
        &self.styles
    }

    pub fn session_id(&self) -> Uuid {
        self.session_id
    }

    pub fn set_session_id(&mut self, session_id: Uuid) {
        self.session_id = session_id;
    }

    /// Прогнать реплику через конвейер целиком.
    ///
    /// Запись приходит по значению: дальше она конвейеру и принадлежит, а вызывающему
    /// незачем держать в памяти второй такой буфер.
    #[allow(clippy::needless_pass_by_value)]
    pub fn run(
        &mut self,
        audio: PcmAudio,
        mode: Mode,
        style: Option<&str>,
        app_hint: Option<&str>,
    ) -> Result<Entry, PipelineError> {
        let started = self.clock.instant();
        let audio = audio.to_16k();
        // Куски, распознанные во время записи; здесь на входе тогда только хвост реплики, а её
        // длительность знает префикс.
        let prefix = self.chunk_prefix.take().unwrap_or_default();
        let audio_secs = prefix.audio_secs.unwrap_or_else(|| audio.duration_secs());

        // Выделение забирается до распознавания: пока пользователь говорил, оно ещё на месте.
        let selection = match mode {
            Mode::Command => {
                if !self.config.command_mode.enabled {
                    return Err(PipelineError::CommandModeDisabled);
                }
                Some(
                    self.injector
                        .copy_selection()
                        .map_err(|err| PipelineError::Selection(err.to_string()))?,
                )
            }
            Mode::Dictation => None,
        };

        let style = self.styles.resolve(style, app_hint, &self.config.style);

        let options = self.stt_options();
        let stt_started = self.clock.instant();
        let transcript = self.transcribe_utterance(&audio, &options, &prefix)?;
        let stt_ms = millis_since(stt_started, self.clock.instant()) + prefix.stt_ms;

        // Тишина и шум: whisper уверенно печатает «Продолжение следует» на пустом входе.
        // Такую реплику не вставляем и не отдаём в модель, но в журнал она попадает (F-22).
        let text_raw = if is_silence_hallucination(&transcript, self.config.stt.no_speech_threshold)
        {
            String::new()
        } else {
            transcript.text.trim().to_string()
        };
        let language = transcript
            .detected_language
            .clone()
            .or_else(|| match &options.language {
                LanguageHint::Fixed(code) => Some(code.clone()),
                LanguageHint::Auto => None,
            });
        let lang = language.clone().unwrap_or_else(|| "auto".to_string());

        let rules_started = self.clock.instant();
        let (after_dictionary, dict_hits) = self.dictionary.apply(&text_raw);
        let after_rules = match mode {
            // Инструкция режима команд уходит в модель как есть: правила исказили бы команду.
            Mode::Command => after_dictionary,
            Mode::Dictation => self.rules.apply(&after_dictionary, &lang),
        };
        let rules_ms = millis_since(rules_started, self.clock.instant());

        let mut entry = Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: self.clock.now_utc(),
            session_id: self.session_id,
            mode,
            source: Source::Mic,
            app: app_hint.map(ToString::to_string),
            language,
            audio_secs,
            words: 0,
            wpm: None,
            style: style.id.clone(),
            stt_engine: self.stt.id().to_string(),
            stt_model: self.stt.model_name().to_string(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: self.llm.as_ref().is_none_or(|llm| llm.is_local()),
            dict_hits,
            inject_method: None,
            latency_ms: LatencyMs {
                stt: stt_ms,
                rules: rules_ms,
                stop_after_release: self.stop_after_release_ms.take(),
                first_hypothesis: prefix.first_hypothesis_ms,
                ..Default::default()
            },
            tokens: None,
            error: None,
            text_raw: Some(text_raw),
            text_final: None,
            audio_path: None,
        };

        let text = match mode {
            Mode::Command => {
                let selection = selection.unwrap_or_default();
                self.run_command(&mut entry, &selection, &after_rules)?
            }
            Mode::Dictation => self.run_dictation(&mut entry, &style, after_rules),
        };

        entry.words = word_count(&text);
        entry.wpm = Entry::wpm_for(entry.words, audio_secs);
        entry.text_final = Some(text.clone());

        if text.trim().is_empty() {
            // Вставлять нечего: тишина или галлюцинация на пустом аудио.
            entry.error = Some("пустая реплика: вставлять нечего".into());
            warn!("реплика пустая после постобработки, вставка пропущена");
        } else {
            let resolved = output_mode(&self.config.output.mode)
                .resolve(&text, self.config.output.auto_type_max_chars as usize);
            // Способ вставки должен знать класс окна: в терминалах вставка идёт Ctrl+Shift+V.
            self.injector.set_window(app_hint);
            self.wait_before_inject();
            let inject_started = self.clock.instant();
            match self.injector.inject(&text, resolved) {
                Ok(report) => {
                    entry.inject_method = Some(inject_method_for(&report.method, app_hint));
                    entry.latency_ms.inject =
                        Some(millis_since(inject_started, self.clock.instant()));
                }
                Err(err) => {
                    warn!(error = %err, "вставка не удалась, текст остаётся в буфере");
                    entry.error = Some(err.to_string());
                }
            }
        }

        entry.latency_ms.total = millis_since(started, self.clock.instant());

        if self.config.privacy.no_record_mode {
            // Режим «не записывать»: журнал не трогаем вовсе.
            return Ok(entry);
        }
        self.journal.append(&entry)?;
        Ok(entry)
    }

    /// Пауза перед вставкой, задаваемая настройкой `output.pre_inject_delay_ms`.
    ///
    /// Между отпусканием клавиши и вставкой окно должно успеть вернуть фокус в поле ввода.
    /// Локально хватает 50 мс по умолчанию; на удалённом рабочем столе и в медленных приложениях
    /// пауза ставится вручную вплоть до 1500 мс, иначе первые символы уходят в никуда.
    fn wait_before_inject(&self) {
        let delay = self.config.output.pre_inject_delay_ms;
        if delay == 0 {
            return;
        }
        self.clock
            .sleep(std::time::Duration::from_millis(u64::from(delay)));
    }

    /// Распознать кусок ещё идущей записи (потоковая обработка, см. [`chunked`]).
    ///
    /// Куски копит демон, а конвейер только читает их моделью — со словарём, языком реплики и
    /// хвостом предыдущего текста в подсказке.
    pub fn transcribe_chunk(
        &mut self,
        audio: &PcmAudio,
        context: &ChunkContext,
    ) -> Result<ChunkText, SttError> {
        let options = self.stt_options();
        let started = self.clock.instant();
        let mut chunk = chunked::transcribe_chunk(
            self.stt.as_mut(),
            &audio.to_16k(),
            &options,
            context,
            self.config.stt.no_speech_threshold,
        )?;
        chunk.stt_ms = millis_since(started, self.clock.instant());
        Ok(chunk)
    }

    /// Отдать конвейеру начало реплики, распознанное кусками: следующий [`Pipeline::run`] получит
    /// только хвост.
    pub fn set_chunk_prefix(&mut self, prefix: ChunkPrefix) {
        self.chunk_prefix = (!prefix.is_empty()).then_some(prefix);
    }

    /// Распознать реплику целиком или, если начало уже есть кусками, только её хвост.
    fn transcribe_utterance(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
        prefix: &ChunkPrefix,
    ) -> Result<Transcript, SttError> {
        if prefix.is_empty() {
            return transcribe_with_language_policy(self.stt.as_mut(), audio, options);
        }
        let tail = if audio.samples.is_empty() {
            // Последний кусок совпал с концом реплики: распознавать нечего, всё уже готово.
            None
        } else {
            let options = chunked::tail_options(options, prefix);
            Some(transcribe_with_language_policy(
                self.stt.as_mut(),
                audio,
                &options,
            )?)
        };
        Ok(chunked::merge(prefix, tail))
    }

    /// Параметры распознавания из настроек и словаря.
    fn stt_options(&self) -> SttOptions {
        let initial_prompt = if self.config.dictionary.in_prompt {
            let hint = self.dictionary.prompt_hint();
            (!hint.is_empty()).then_some(hint)
        } else {
            None
        };
        SttOptions {
            language: LanguageHint::parse(&self.config.stt.language),
            allowed_languages: self.config.stt.allowed_languages.clone(),
            initial_prompt,
            threads: self.config.stt.threads as usize,
            timestamps: false,
        }
    }

    /// Нужна ли модель для этой реплики.
    fn wants_llm(&self, style_uses_llm: bool, text: &str) -> bool {
        style_uses_llm
            && self.config.llm.enabled
            && self.config.privacy.send_to_llm
            && self.llm.is_some()
            && word_count(text) > self.config.rules.llm_min_words
    }

    /// Диктовка: правила плюс, при необходимости, модель.
    fn run_dictation(
        &mut self,
        entry: &mut Entry,
        style: &crate::domain::text::Style,
        after_rules: String,
    ) -> String {
        if !self.wants_llm(style.uses_llm, &after_rules) {
            return after_rules;
        }
        let request = ChatRequest {
            model: self.config.llm.model.clone(),
            system: style.system_prompt.clone(),
            user: after_rules.clone(),
            temperature: self.config.llm.temperature,
            max_tokens: self.config.llm.max_tokens,
        };
        match self.complete_with_retries(&request) {
            Ok((text, tokens, elapsed_ms)) => {
                self.record_llm(entry, tokens, elapsed_ms);
                text
            }
            Err(err) => {
                // Реплика не теряется: возвращаем текст после правил.
                warn!(error = %err, "постобработка не удалась, отдаю текст после правил");
                after_rules
            }
        }
    }

    /// Режим команд: инструкция применяется к выделенному тексту.
    fn run_command(
        &mut self,
        entry: &mut Entry,
        selection: &str,
        instruction: &str,
    ) -> Result<String, PipelineError> {
        if self.llm.is_none() || !self.config.llm.enabled {
            return Err(PipelineError::CommandModeNeedsLlm(LlmError::Disabled));
        }
        if !self.config.privacy.send_to_llm {
            return Err(PipelineError::CommandModeNeedsLlm(LlmError::Disabled));
        }
        let request = ChatRequest {
            model: self.config.llm.model.clone(),
            system: self.config.command_mode.system_prompt.clone(),
            // Сначала инструкция, потом текст: так модель не принимает фрагмент за задание.
            // Ответ должен быть только текстом — без заголовков, пояснений и кавычек.
            user: format!(
                "ИНСТРУКЦИЯ: {instruction}\n\nТЕКСТ:\n{selection}\n\n\
                 Верни только текст после применения инструкции, одной репликой, без \
                 заголовков, пояснений, кавычек и markdown."
            ),
            temperature: self.config.llm.temperature,
            max_tokens: self.config.llm.max_tokens,
        };
        match self.complete_with_retries(&request) {
            Ok((text, tokens, elapsed_ms)) => {
                self.record_llm(entry, tokens, elapsed_ms);
                Ok(text)
            }
            Err(err) => Err(PipelineError::CommandModeNeedsLlm(err)),
        }
    }

    /// Запрос к модели с ограниченным числом повторов.
    ///
    /// Ошибка авторизации не повторяется: ключ от повтора не починится.
    fn complete_with_retries(
        &self,
        request: &ChatRequest,
    ) -> Result<(String, Option<Tokens>, u32), LlmError> {
        let Some(llm) = self.llm.as_ref() else {
            return Err(LlmError::Disabled);
        };
        let attempts = self.config.llm.max_retries.saturating_add(1);
        let started = self.clock.instant();
        let mut last = LlmError::Disabled;
        for attempt in 1..=attempts {
            match llm.complete(request) {
                Ok(response) => {
                    let tokens = match (response.prompt_tokens, response.completion_tokens) {
                        (None, None) => None,
                        (prompt, completion) => Some(Tokens {
                            prompt: prompt.unwrap_or(0),
                            completion: completion.unwrap_or(0),
                        }),
                    };
                    // Служебные теги, ограждения, вводные фразы и эхо словаря не должны
                    // попасть в поле пользователя (AM-19).
                    let text = sanitize_llm_output(&response.text, &self.dictionary.prompt_hint());
                    if text.is_empty() {
                        last = LlmError::BadResponse("модель вернула пустой текст".into());
                        continue;
                    }
                    return Ok((text, tokens, millis_since(started, self.clock.instant())));
                }
                Err(LlmError::Auth) => return Err(LlmError::Auth),
                Err(err) => {
                    warn!(attempt, attempts, error = %err, "повтор запроса к модели");
                    last = err;
                }
            }
        }
        Err(last)
    }

    /// Отметить в записи, что модель отработала.
    fn record_llm(&self, entry: &mut Entry, tokens: Option<Tokens>, elapsed_ms: u32) {
        entry.llm_used = true;
        entry.llm_provider = self.llm.as_ref().map(|llm| llm.id().to_string());
        entry.llm_model = Some(self.config.llm.model.clone());
        entry.local_llm = self.llm.as_ref().is_some_and(|llm| llm.is_local());
        entry.tokens = tokens;
        entry.latency_ms.llm = Some(elapsed_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::app::dictionary::Term;
    use crate::domain::fakes::{FakeClock, FakeLlm, FakeStt, MemJournal, RecordingInjector};
    use crate::domain::inject::{InjectError, InjectReport};
    use crate::domain::journal::Journal as JournalTrait;
    use crate::domain::stt::Transcript;

    /// Фейк, к которому у теста остаётся доступ после передачи в конвейер.
    #[derive(Debug)]
    struct SharedInjector(Arc<Mutex<RecordingInjector>>);

    impl TextInjector for SharedInjector {
        fn id(&self) -> &'static str {
            "shared"
        }

        fn available(&self) -> bool {
            true
        }

        fn inject(&mut self, text: &str, mode: OutputMode) -> Result<InjectReport, InjectError> {
            self.0.lock().unwrap().inject(text, mode)
        }

        fn copy_selection(&mut self) -> Result<String, InjectError> {
            self.0.lock().unwrap().copy_selection()
        }
    }

    #[derive(Debug)]
    struct SharedJournal(Arc<Mutex<MemJournal>>);

    impl JournalTrait for SharedJournal {
        fn append(&mut self, entry: &Entry) -> Result<(), JournalError> {
            self.0.lock().unwrap().append(entry)
        }
    }

    #[derive(Debug)]
    struct SharedStt(Arc<Mutex<FakeStt>>);

    impl SttEngine for SharedStt {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn model_name(&self) -> &'static str {
            "fake"
        }

        fn transcribe(
            &mut self,
            audio: &PcmAudio,
            opts: &SttOptions,
        ) -> Result<Transcript, SttError> {
            self.0.lock().unwrap().transcribe(audio, opts)
        }

        fn unload(&mut self) {
            self.0.lock().unwrap().unload();
        }
    }

    struct Harness {
        pipeline: Pipeline,
        injector: Arc<Mutex<RecordingInjector>>,
        journal: Arc<Mutex<MemJournal>>,
        stt: Arc<Mutex<FakeStt>>,
        clock: Arc<FakeClock>,
    }

    fn audio(secs: f32) -> PcmAudio {
        PcmAudio::new(vec![0.1; (secs * 16_000.0) as usize], 16_000)
    }

    fn clock() -> Arc<FakeClock> {
        Arc::new(FakeClock::at(
            chrono::DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ))
    }

    fn build(transcript: &str, llm: Option<Arc<FakeLlm>>, config: PipelineConfig) -> Harness {
        let stt = Arc::new(Mutex::new(FakeStt::returning(transcript)));
        let injector = Arc::new(Mutex::new(RecordingInjector::default()));
        let journal = Arc::new(Mutex::new(MemJournal::default()));
        let clock = clock();
        let pipeline = Pipeline::new(
            Box::new(SharedStt(Arc::clone(&stt))),
            llm.map(|llm| llm as Arc<dyn LlmClient>),
            Box::new(SharedInjector(Arc::clone(&injector))),
            Box::new(SharedJournal(Arc::clone(&journal))),
            clock.clone(),
            config,
        );
        Harness {
            pipeline,
            injector,
            journal,
            stt,
            clock,
        }
    }

    /// Настройки с включённой моделью: по умолчанию `llm.enabled = false`.
    fn with_llm() -> PipelineConfig {
        let mut config = PipelineConfig::default();
        config.llm.enabled = true;
        config
    }

    fn injected(harness: &Harness) -> Vec<String> {
        harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(text, _)| text.clone())
            .collect()
    }

    #[test]
    fn the_injector_receives_the_text_after_the_dictionary_and_the_rules() {
        let mut harness = build(
            "ну молва запятая это работает точка",
            None,
            PipelineConfig::default(),
        );
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();

        assert_eq!(injected(&harness), vec!["MolvAI, это работает."]);
        assert_eq!(entry.text_final.as_deref(), Some("MolvAI, это работает."));
        assert_eq!(
            entry.text_raw.as_deref(),
            Some("ну молва запятая это работает точка")
        );
        assert_eq!(entry.dict_hits, 1);
        assert_eq!(entry.app.as_deref(), Some("kitty"));
        assert!(!entry.llm_used);
    }

    #[test]
    fn a_failing_model_still_delivers_the_text_after_the_rules() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Timeout(20)));
        let text = "это достаточно длинная реплика из более чем десяти слов запятая \
                    чтобы модель точно вызвалась точка";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert!(llm.calls() >= 1, "модель должна была вызваться");
        assert!(!entry.llm_used);
        assert_eq!(entry.error, None, "отказ модели не ошибка реплики");
        assert_eq!(injected(&harness).len(), 1);
        assert!(
            injected(&harness)[0].ends_with('.'),
            "{:?}",
            injected(&harness)
        );
        assert!(entry.tokens.is_none());
        assert!(entry.latency_ms.llm.is_none());
    }

    #[test]
    fn a_short_utterance_never_reaches_the_model() {
        let llm = Arc::new(FakeLlm::echoing("переписано моделью"));
        let mut harness = build("привет мир как дела", Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(3.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
        assert_eq!(injected(&harness), vec!["Привет мир как дела"]);
    }

    #[test]
    fn a_long_utterance_goes_through_the_model_and_records_tokens() {
        let llm = Arc::new(FakeLlm::echoing("Переписанный моделью текст."));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 1);
        assert!(entry.llm_used);
        assert_eq!(entry.llm_provider.as_deref(), Some("fake"));
        assert_eq!(entry.llm_model.as_deref(), Some("qwen3.5:4b"));
        assert!(entry.local_llm);
        assert_eq!(
            entry.tokens,
            Some(Tokens {
                prompt: 10,
                completion: 5
            })
        );
        assert!(entry.latency_ms.llm.is_some());
        assert_eq!(injected(&harness), vec!["Переписанный моделью текст."]);
    }

    #[test]
    fn the_verbatim_style_never_reaches_the_model() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, Some("verbatim"), None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert_eq!(entry.style, "verbatim");
    }

    #[test]
    fn privacy_forbids_sending_anything_to_the_model() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let mut config = with_llm();
        config.privacy.send_to_llm = false;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
        assert!(!entry.llm_used);
    }

    #[test]
    fn a_disabled_model_leaves_the_rules_in_charge() {
        let llm = Arc::new(FakeLlm::echoing("переписано"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        // llm.enabled = false в настройках по умолчанию.
        let mut harness = build(text, Some(Arc::clone(&llm)), PipelineConfig::default());
        harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 0);
    }

    #[test]
    fn retries_are_bounded_by_the_configuration() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Unavailable("лежит".into())));
        let mut config = with_llm();
        config.llm.max_retries = 2;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 3, "одна попытка плюс два повтора");
    }

    #[test]
    fn a_rejected_key_is_not_retried() {
        let llm = Arc::new(FakeLlm::failing(LlmError::Auth));
        let mut config = with_llm();
        config.llm.max_retries = 3;
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), config);
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 1, "ключ от повтора не починится");
        assert!(!entry.llm_used);
    }

    #[test]
    fn the_journal_gets_one_entry_with_all_the_metrics() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();
        let stored = harness.journal.lock().unwrap().entries.clone();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, entry.id);
        assert_eq!(stored[0].session_id, harness.pipeline.session_id());
        assert_eq!(stored[0].words, 2);
        assert_eq!(stored[0].wpm, Entry::wpm_for(2, 4.0));
        assert!((stored[0].audio_secs - 4.0).abs() < 1e-3);
        assert_eq!(stored[0].stt_engine, "fake");
        assert!(stored[0].inject_method.is_some());
    }

    #[test]
    fn no_record_mode_keeps_the_journal_empty_but_still_injects() {
        let mut config = PipelineConfig::default();
        config.privacy.no_record_mode = true;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.journal.lock().unwrap().entries.is_empty());
        assert_eq!(injected(&harness), vec!["Привет мир"]);
    }

    #[test]
    fn a_failed_injection_lands_in_the_entry_and_the_journal() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness.injector.lock().unwrap().fail_with =
            Some(InjectError::Failed("нет активного окна".into()));
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(
            entry.error.is_some(),
            "ошибка вставки должна попасть в запись"
        );
        assert!(entry.error.unwrap().contains("нет активного окна"));
        assert_eq!(entry.text_final.as_deref(), Some("Привет мир"));
        assert_eq!(harness.journal.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn an_empty_transcript_is_not_injected_but_is_recorded() {
        let mut harness = build("   ", None, PipelineConfig::default());
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(injected(&harness).is_empty());
        assert_eq!(entry.words, 0);
        assert!(entry.error.is_some());
        assert_eq!(harness.journal.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn stt_options_carry_the_language_and_the_dictionary_hint() {
        let mut config = PipelineConfig::default();
        config.stt.language = "ru".into();
        let mut harness = build("привет", None, config);
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        harness
            .pipeline
            .run(audio(2.0), Mode::Dictation, None, None)
            .unwrap();
        let calls = harness.stt.lock().unwrap().calls.clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].language, LanguageHint::Fixed("ru".into()));
        assert_eq!(calls[0].initial_prompt.as_deref(), Some("MolvAI"));
        assert_eq!(calls[0].allowed_languages, vec!["ru", "en"]);
    }

    #[test]
    fn the_dictionary_hint_can_be_switched_off() {
        let mut config = PipelineConfig::default();
        config.dictionary.in_prompt = false;
        let mut harness = build("привет", None, config);
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        harness
            .pipeline
            .run(audio(2.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(harness.stt.lock().unwrap().calls[0].initial_prompt, None);
    }

    #[test]
    fn the_window_class_picks_the_style_when_nothing_is_requested() {
        let mut config = with_llm();
        config
            .style
            .by_app
            .insert("kitty".into(), "verbatim".into());
        let mut harness = build("привет мир", None, config);
        let entry = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, Some("kitty"))
            .unwrap();
        assert_eq!(entry.style, "verbatim");
    }

    #[test]
    fn command_mode_applies_the_instruction_to_the_selection() {
        let llm = Arc::new(FakeLlm::echoing("Исправленный выделенный текст."));
        let mut harness = build(
            "сделай текст официальным",
            Some(Arc::clone(&llm)),
            with_llm(),
        );
        harness.injector.lock().unwrap().selection = Some("привет как сам".into());

        let entry = harness
            .pipeline
            .run(audio(3.0), Mode::Command, None, None)
            .unwrap();

        assert_eq!(llm.calls(), 1);
        let request = llm.last_request.lock().unwrap().clone().unwrap();
        assert!(request.user.contains("привет как сам"), "{}", request.user);
        assert!(
            request.user.contains("сделай текст официальным"),
            "{}",
            request.user
        );
        // Промпт по умолчанию велит вернуть только текст: без заголовков и пояснений.
        assert!(request.system.contains("только получившийся текст"), "{}", request.system);
        assert_eq!(injected(&harness), vec!["Исправленный выделенный текст."]);
        assert_eq!(entry.mode, Mode::Command);
        assert!(entry.llm_used);
    }

    #[test]
    fn command_mode_is_short_even_though_the_model_is_still_called() {
        // Порог llm_min_words к режиму команд не применяется: без модели он бессмыслен.
        let llm = Arc::new(FakeLlm::echoing("Готово."));
        let mut harness = build("покороче", Some(Arc::clone(&llm)), with_llm());
        harness.injector.lock().unwrap().selection = Some("длинный выделенный текст".into());
        harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap();
        assert_eq!(llm.calls(), 1);
    }

    #[test]
    fn command_mode_without_a_model_is_an_explicit_error() {
        let mut harness = build("сделай короче", None, PipelineConfig::default());
        harness.injector.lock().unwrap().selection = Some("текст".into());
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(
            matches!(err, PipelineError::CommandModeNeedsLlm(_)),
            "{err:?}"
        );
    }

    #[test]
    fn command_mode_without_a_selection_is_an_explicit_error() {
        let llm = Arc::new(FakeLlm::echoing("ок"));
        let mut harness = build("сделай короче", Some(llm), with_llm());
        harness.injector.lock().unwrap().selection = None;
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::Selection(_)), "{err:?}");
    }

    #[test]
    fn command_mode_can_be_switched_off() {
        let llm = Arc::new(FakeLlm::echoing("ок"));
        let mut config = with_llm();
        config.command_mode.enabled = false;
        let mut harness = build("сделай короче", Some(llm), config);
        let err = harness
            .pipeline
            .run(audio(2.0), Mode::Command, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::CommandModeDisabled), "{err:?}");
    }

    #[test]
    fn a_recognition_failure_is_an_error_and_nothing_is_injected() {
        let stt = Arc::new(Mutex::new(FakeStt::with_responses(vec![Err(
            SttError::ModelLoad("нет файла модели".into()),
        )])));
        let injector = Arc::new(Mutex::new(RecordingInjector::default()));
        let journal = Arc::new(Mutex::new(MemJournal::default()));
        let mut pipeline = Pipeline::new(
            Box::new(SharedStt(Arc::clone(&stt))),
            None,
            Box::new(SharedInjector(Arc::clone(&injector))),
            Box::new(SharedJournal(Arc::clone(&journal))),
            clock(),
            PipelineConfig::default(),
        );
        let err = pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap_err();
        assert!(matches!(err, PipelineError::Stt(_)), "{err:?}");
        assert!(injector.lock().unwrap().injected.is_empty());
        assert!(journal.lock().unwrap().entries.is_empty());
    }

    #[test]
    fn the_output_mode_from_the_configuration_reaches_the_injector() {
        let mut config = PipelineConfig::default();
        config.output.mode = "clipboard".into();
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        let modes: Vec<OutputMode> = harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(_, mode)| *mode)
            .collect();
        assert_eq!(modes, vec![OutputMode::Clipboard]);
    }

    #[test]
    fn auto_output_mode_is_resolved_before_the_injector_sees_it() {
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        let modes: Vec<OutputMode> = harness
            .injector
            .lock()
            .unwrap()
            .injected
            .iter()
            .map(|(_, mode)| *mode)
            .collect();
        assert_eq!(modes, vec![OutputMode::Type], "короткий текст набирается");
    }

    #[test]
    fn service_tags_from_the_model_never_reach_the_injector() {
        // Критерий AM-19: рассуждения и ограждения остаются у модели, в поле идёт текст.
        let llm = Arc::new(FakeLlm::echoing(
            "<think>подумаю ещё</think>\n```\nСобрание переносится на среду.\n```",
        ));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(llm), with_llm());
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(injected(&harness), vec!["Собрание переносится на среду."]);
        assert_eq!(
            entry.text_final.as_deref(),
            Some("Собрание переносится на среду.")
        );
    }

    #[test]
    fn command_mode_output_is_cleaned_up_too() {
        let llm = Arc::new(FakeLlm::echoing(
            "Вот исправленный текст: \"Добрый день, коллеги.\"",
        ));
        let mut harness = build("сделай официальным", Some(llm), with_llm());
        harness.injector.lock().unwrap().selection = Some("привет как сам".into());
        harness
            .pipeline
            .run(audio(3.0), Mode::Command, None, None)
            .unwrap();
        assert_eq!(injected(&harness), vec!["Добрый день, коллеги."]);
    }

    #[test]
    fn a_model_that_echoes_the_dictionary_is_treated_as_a_failure() {
        // Ответ, состоящий из одной подсказки словаря, — это не реплика: остаётся текст правил.
        let llm = Arc::new(FakeLlm::echoing("MolvAI"));
        let text = "это достаточно длинная реплика из более чем десяти слов чтобы модель \
                    точно вызвалась";
        let mut harness = build(text, Some(Arc::clone(&llm)), with_llm());
        harness.pipeline.set_dictionary(Dictionary::from_terms(
            vec![Term::new("MolvAI", &["молва"])],
            false,
        ));
        let entry = harness
            .pipeline
            .run(audio(8.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(!entry.llm_used, "эхо словаря — не ответ модели");
        assert!(
            injected(&harness)[0].starts_with("Это достаточно длинная"),
            "{:?}",
            injected(&harness)
        );
    }

    #[test]
    fn the_pause_before_the_injection_comes_from_the_configuration() {
        // Критерий AM-20: пауза перед вставкой задаётся настройкой, а не зашита в код.
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(
            harness.clock.slept(),
            vec![std::time::Duration::from_millis(50)],
            "по умолчанию перед вставкой ждём 50 мс"
        );

        // Удалённому рабочему столу нужно больше времени на возврат фокуса.
        let mut config = PipelineConfig::default();
        config.output.pre_inject_delay_ms = 1500;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(
            harness.clock.slept(),
            vec![std::time::Duration::from_millis(1500)]
        );
    }

    #[test]
    fn the_microphone_release_measurement_lands_in_the_entry_once() {
        // Гарантия приватности микрофона должна быть видна в журнале: замер демона попадает
        // в запись той реплики, к которой относится, и не переезжает в следующую.
        let mut harness = build("привет мир", None, PipelineConfig::default());
        harness.pipeline.set_stop_after_release(120);
        let first = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(first.latency_ms.stop_after_release, Some(120));
        let second = harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert_eq!(second.latency_ms.stop_after_release, None);
    }

    #[test]
    fn a_zero_pause_does_not_wait_at_all() {
        let mut config = PipelineConfig::default();
        config.output.pre_inject_delay_ms = 0;
        let mut harness = build("привет мир", None, config);
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.clock.slept().is_empty());
    }

    #[test]
    fn nothing_to_inject_means_nothing_to_wait_for() {
        // Пустая реплика не вставляется — и паузы перед вставкой тоже быть не должно.
        let mut harness = build("   ", None, PipelineConfig::default());
        harness
            .pipeline
            .run(audio(4.0), Mode::Dictation, None, None)
            .unwrap();
        assert!(harness.clock.slept().is_empty());
    }

    #[test]
    fn without_an_active_window_the_journal_says_the_text_stayed_in_the_clipboard() {
        // Критерий AJ-09/AJ-10: поля ввода нет — текст в буфере, и в истории это видно.
        assert_eq!(inject_method_for("clipboard-only", None), NO_FOCUS_METHOD);
        assert_eq!(
            inject_method_for("clipboard-only", Some("kitty")),
            "clipboard-only",
            "окно есть: обычная вставка через буфер, пусть и с Ctrl+V руками"
        );
        assert_eq!(inject_method_for("wtype-type", None), "wtype-type");
    }

    #[test]
    fn output_mode_parsing_falls_back_to_auto() {
        assert_eq!(output_mode("paste"), OutputMode::Paste);
        assert_eq!(output_mode(" TYPE "), OutputMode::Type);
        assert_eq!(output_mode("clipboard"), OutputMode::Clipboard);
        assert_eq!(output_mode("что-то не то"), OutputMode::Auto);
    }
}
