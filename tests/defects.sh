#!/usr/bin/env bash
# Список дефектов для `scripts/t.sh falsify` (`just falsify`).
#
#   defect NAME FILE FIND REPLACE CONSEQUENCE
#
# Каждая запись нейтрализует одну гарантию, не ломая компиляцию, и сьют обязан это заметить.
# Запись добавляется вместе с гарантией, которую она проверяет. FIND встречается в FILE ровно
# один раз, иначе харнесс сообщает `stale` — список отстал от кода.

defect 'audio/mono-downmix-averages' 'crates/molva-core/src/domain/audio.rs' \
  '            sum / channels as f32' \
  '            sum' \
  'стерео сводится в моно с удвоенной громкостью: клиппинг и ложные уровни сигнала'

defect 'audio/resample-changes-rate' 'crates/molva-core/src/domain/audio.rs' \
  '    if audio.sample_rate == target_rate || audio.samples.is_empty() || audio.sample_rate == 0 {' \
  '    if true {' \
  'аудио 48 кГц уходит в whisper как 16 кГц: речь в три раза быстрее, распознавание разваливается'

defect 'entry/wpm-short-audio' 'crates/molva-core/src/domain/entry.rs' \
  '        if audio_secs < MIN_AUDIO_SECS_FOR_WPM {' \
  '        if audio_secs < 0.0 {' \
  'реплика в полсекунды даёт абсурдный WPM и становится личным рекордом'

defect 'entry/privacy-keeps-text' 'crates/molva-core/src/domain/entry.rs' \
  '        self.text_final = None;' \
  '        self.text_final = self.text_final.take();' \
  'в режиме приватности итоговый текст всё равно уходит в журнал'

defect 'text/word-count-punctuation' 'crates/molva-core/src/domain/text.rs' \
  '        .filter(|token| token.chars().any(char::is_alphanumeric))' \
  '        .filter(|_token| true)' \
  'тире и знаки препинания считаются словами: WPM и счётчики слов завышены'

defect 'inject/auto-threshold' 'crates/molva-core/src/domain/inject.rs' \
  '            OutputMode::Auto if text.chars().count() <= auto_type_max_chars => OutputMode::Type,' \
  '            OutputMode::Auto if text.len() <= auto_type_max_chars => OutputMode::Type,' \
  'порог auto считает байты, а не символы: кириллица уходит в paste вдвое раньше, чем настроено'

defect 'daemon/min-hold-discards-audio' 'crates/molva-core/src/app/daemon/state.rs' \
  '        if !rec.latched && held < Self::ms(self.hotkeys.min_hold_ms) {' \
  '        if !rec.latched && held < Duration::ZERO {' \
  'случайный чирк по клавише запускает распознавание полусекунды тишины и вставляет мусор'

defect 'daemon/second-start-is-busy' 'crates/molva-core/src/app/daemon/state.rs' \
  '                Outcome::busy("запись уже идёт")' \
  '                Outcome::nothing()' \
  'второе нажатие во время записи молча ничего не делает: пользователь не понимает, что демон занят'

defect 'daemon/toggle-stops-recording' 'crates/molva-core/src/app/daemon/state.rs' \
  '    fn toggle_off(&mut self, at: Instant) -> Outcome {' \
  '    fn toggle_off(&mut self, at: Instant) -> Outcome { self.last_press = Some(at); return Outcome::nothing();' \
  'hands-free нельзя выключить: toggle включает запись и больше не выключает её'

defect 'daemon/journal-records-every-reply' 'crates/molva-core/src/app/daemon/processor.rs' \
  '        self.journal.append(&stored)?;' \
  '        let _ = &stored;' \
  'реплики не попадают в журнал: истории и статистики нет, хотя всё «работает»'

defect 'inject/chain-falls-back-to-clipboard' 'crates/molva-core/src/infra/inject/chain.rs' \
  '        let mut report = self.fallback.inject(text, OutputMode::Clipboard)?;' \
  '        let mut report = InjectReport::default();' \
  'когда все способы вставки отказали, реплика теряется вместо того, чтобы лечь в буфер обмена'

defect 'inject/terminal-needs-shift' 'crates/molva-core/src/infra/inject/chain.rs' \
  '        let terminal = class.is_some_and(is_terminal_class);' \
  '        let terminal = false;' \
  'в терминале уходит Ctrl+V, который там ничего не вставляет: реплика молча пропадает'

defect 'daemon/window-reaches-injector' 'crates/molva-core/src/app/daemon/processor.rs' \
  '            self.injector.set_window(app_hint);' \
  '            self.injector.set_window(None);' \
  'способ вставки не знает, куда вставляет: в терминалах и в браузере он одинаково неверный'

defect 'inject/clipboard-is-restored' 'crates/molva-core/src/infra/inject/clipboard.rs' \
  '        self.backend.restore(&saved)' \
  '        Ok(())' \
  'скопированная пользователем ссылка навсегда затирается текстом реплики'

defect 'ipc/unknown-command-is-refused' 'crates/molva-core/src/infra/ipc/transport.rs' \
  '                    &Response::err(0, ErrorCode::BadRequest, err.to_string(), None),' \
  '                    &Response::ok(0, Value::Null),' \
  'битый запрос получает «ок»: клиент считает, что команда выполнена, а демон её не понял'

defect 'ipc/stale-socket-is-removed' 'crates/molva-core/src/infra/ipc/transport.rs' \
  '        if path_is_stale(path) {' \
  '        if false {' \
  'после сбоя демон больше не поднимается: файл сокета остался, адрес занят'

defect 'platform/hyprland-is-detected' 'crates/molva-core/src/infra/platform.rs' \
  '    if env.hyprland_signature.is_some() {' \
  '    if false {' \
  'на Hyprland выбирается чужая цепочка вставки: hyprctl не пробуется вообще'

defect 'config/missing-file-is-default' 'crates/molva-core/src/config.rs' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ConfigError::Read { path: path.to_path_buf(), source: e }),' \
  'первый запуск без файла настроек падает вместо того, чтобы работать со значениями по умолчанию'

defect 'audio/trim-silence-threshold' 'crates/molva-core/src/app/audio/trim.rs' \
  '        .filter(|(_, chunk)| amplitude_to_db(rms(chunk)) >= threshold_db)' \
  '        .filter(|(_, chunk)| !chunk.is_empty())' \
  'тишина уходит в whisper целиком: модель галлюцинирует текст там, где никто не говорил'

defect 'audio/trim-keep-ms' 'crates/molva-core/src/app/audio/trim.rs' \
  '    let keep = (u64::from(audio.sample_rate) * u64::from(keep_ms) / 1000) as usize;' \
  '    let keep = 0;' \
  'запас keep_ms не оставлен: обрезка съедает первый слог реплики'

defect 'stt/language-retry' 'crates/molva-core/src/infra/stt/mod.rs' \
  '    if options.language != LanguageHint::Auto {' \
  '    if true {' \
  'русская речь, принятая за украинскую, уходит в текст как есть: повтора с разрешённым языком нет'

defect 'stt/model-missing-message' 'crates/molva-core/src/infra/stt/whisper.rs' \
  '            if !self.model_path.exists() {' \
  '            if false {' \
  'вместо «скачайте модель» пользователь получает невнятную ошибку загрузки весов'

defect 'stt/silence-gate' 'crates/molva-core/src/infra/stt/mod.rs' \
  '        .is_some_and(|p| p >= no_speech_threshold)' \
  '        .is_some_and(|p| p >= 1.1)' \
  'на тишине whisper галлюцинирует, и выдуманный текст вставляется в активное поле'

defect 'audio/gain-clamp' 'crates/molva-core/src/infra/audio/cpal_source.rs' \
  '        *sample = (*sample * gain).clamp(-1.0, 1.0);' \
  '        *sample = *sample * gain;' \
  'усиление входа выводит сигнал за пределы диапазона: клиппинг ломает распознавание'

defect 'journal/privacy-strips-text' 'crates/molva-core/src/app/journal.rs' \
  '            owned = entry.clone().without_text();' \
  '            owned = entry.clone();' \
  'в режиме приватности тексты реплик всё равно попадают в файл журнала'

defect 'journal/corrupt-line-quarantined' 'crates/molva-core/src/app/journal.rs' \
  '                    broken.push(line.to_string());' \
  '                    let _ = &broken;' \
  'битая строка молча теряется: пользователь не узнает, что часть истории не читается'

defect 'journal/rotation-keeps-newest' 'crates/molva-core/src/app/journal.rs' \
  '            kept = &kept[kept.len() - max_entries as usize..];' \
  '            kept = &kept[..max_entries as usize];' \
  'ротация оставляет самые старые реплики и выбрасывает свежие'

defect 'journal/owner-only-permissions' 'crates/molva-core/src/app/journal.rs' \
  '        perms.set_mode(0o600);' \
  '        perms.set_mode(0o644);' \
  'журнал с текстами реплик читается любым пользователем системы'

defect 'stats/average-weighted-by-time' 'crates/molva-core/src/app/stats.rs' \
  '    Some(words as f32 / secs * 60.0)' \
  '    Some(entries.iter().filter_map(|e| Entry::wpm_for(e.words, e.audio_secs)).sum::<f32>() / entries.len() as f32)' \
  'средняя скорость усредняется по репликам: одна короткая фраза перевешивает час диктовки'

defect 'stats/record-needs-a-real-utterance' 'crates/molva-core/src/app/stats.rs' \
  '        if entry.audio_secs < RECORD_MIN_AUDIO_SECS || entry.words < RECORD_MIN_WORDS {' \
  '        if false {' \
  'личным рекордом становится полуторасекундная реплика с абсурдным WPM'

defect 'stats/streak-breaks-on-a-gap' 'crates/molva-core/src/app/stats.rs' \
  '    while days.contains(&cursor) {' \
  '    while days.iter().any(|day| *day <= cursor) {' \
  'серия дней подряд не замечает пропущенные дни и растёт до первой записи в истории'

defect 'stats/streak-starts-from-yesterday' 'crates/molva-core/src/app/stats.rs' \
  '        let yesterday = today - Duration::days(1);' \
  '        let yesterday = today - Duration::days(3);' \
  'серия обнуляется утром, до первой реплики нового дня'

defect 'stats/saved-minutes-subtract-dictation' 'crates/molva-core/src/app/stats.rs' \
  '        .map(|e| e.words as f32 / baseline - e.audio_secs / 60.0)' \
  '        .map(|e| e.words as f32 / baseline)' \
  'сэкономленное время не вычитает время самой диктовки и завышено вдвое'

defect 'stats/session-splits-on-a-pause' 'crates/molva-core/src/app/stats.rs' \
  '                    || entry.ts - prev.ts > Duration::minutes(SESSION_GAP_MINUTES)' \
  '                    || false' \
  'часовой перерыв не делит сессию: средняя скорость за сессию размывается простоем'

defect 'stats/reset-marker-hides-old-entries' 'crates/molva-core/src/app/stats.rs' \
  '        Some(at) => entries.iter().filter(|e| e.ts >= at).cloned().collect(),' \
  '        Some(_) => entries.to_vec(),' \
  'сброс статистики ничего не сбрасывает: старые реплики продолжают считаться'

defect 'rules/spoken-full-stop' 'crates/molva-core/src/app/rules.rs' \
  '    ("точка", "."),' \
  '    ("точка", ""),' \
  'продиктованная «точка» пропадает вместо того, чтобы стать знаком препинания'

defect 'rules/point-of-view-is-not-a-command' 'crates/molva-core/src/app/rules.rs' \
  '    if normalize(&tokens[index]) != "точка" && normalize(&tokens[index]) != "точки" {' \
  '    if true {' \
  '«моя точка зрения» превращается в «моя. зрения»'

defect 'rules/intensifier-repeat-survives' 'crates/molva-core/src/app/rules.rs' \
  '"очень", "чуть", "еле", "very"' \
  '' \
  '«очень очень важно» теряет усиление: снятие повторов режет смысл'

defect 'rules/fillers-never-empty-the-text' 'crates/molva-core/src/app/rules.rs' \
  '            .all(|token| !token.chars().any(char::is_alphanumeric))' \
  '            .all(|_token| false)' \
  'реплика из одних заполнителей превращается в пустую строку: сказанное потеряно'

defect 'rules/numerals-need-a-falling-rank' 'crates/molva-core/src/app/rules.rs' \
  '            if found.class >= last_class {' \
  '            if false {' \
  '«три четыре» складывается в «7»: перечисление чисел ломается'

defect 'rules/ordinals-stay-in-words' 'crates/molva-core/src/app/rules.rs' \
  '    if followed_by_ordinal(tokens, at + len) {' \
  '    if false {' \
  '«две тысячи двадцать шестого года» превращается в «2020 шестого года»'

defect 'rules/non-breaking-space-in-units' 'crates/molva-core/src/app/rules.rs' \
  '            out.push(format!("{}\u{a0}{}", tokens[index], tokens[index + 1]));' \
  '            out.push(format!("{} {}", tokens[index], tokens[index + 1]));' \
  '«5 кг» переносится по строкам: число отрывается от единицы измерения'

defect 'rules/dot-inside-a-word-is-not-a-sentence-end' 'crates/molva-core/src/app/rules.rs' \
  '        None => true,' \
  '        None | Some(_) => true,' \
  '`whisper.cpp` превращается в `whisper.Cpp`, а `example.com` — в `example.Com`'

defect 'rules/no-space-inside-a-domain' 'crates/molva-core/src/app/rules.rs' \
  "        if ch == '.' && previous.is_alphanumeric() && !next.is_uppercase() {" \
  '        if false {' \
  'почтовый адрес разводится пробелом: `ivan@example. com` уже не адрес'

defect 'rules/email-is-glued-back' 'crates/molva-core/src/app/rules.rs' \
  '                if is_address_part(&left) && is_address_part(&right) {' \
  '                if false {' \
  'продиктованный адрес остаётся рассыпанным на куски вокруг собачки'

defect 'rules/no-space-before-punctuation' 'crates/molva-core/src/app/rules.rs' \
  "                    '.' | ',' | ';' | ':' | '!' | '?' | ')' | '»' | '…' | '\\u{201d}'" \
  "                    '\\u{0}'" \
  'перед точкой и запятой остаётся пробел: текст выглядит как машинный вывод'

# --- дорожка E: декодирование файлов ---

defect 'decode/downmix-to-mono' 'crates/molva-core/src/infra/audio/decode.rs' \
  '                mono.extend_from_slice(&downmix_to_mono(buf.samples(), channels));' \
  '                mono.extend_from_slice(buf.samples());' \
  'стерео-файл уходит в whisper как чередующиеся каналы: вдвое длиннее и звучит как каша'

defect 'decode/empty-file-rejected' 'crates/molva-core/src/infra/audio/decode.rs' \
  '    if meta.len() == 0 {' \
  '    if false {' \
  'пустой файл вместо понятного «0 байт» даёт ошибку разбора формата'

defect 'decode/native-sample-rate' 'crates/molva-core/src/infra/audio/decode.rs' \
  '    Ok(PcmAudio::new(mono, sample_rate))' \
  '    Ok(PcmAudio::new(mono, 16_000))' \
  'частота файла подменяется на 16 кГц без ресемплинга: длительность и скорость речи врут'

# --- дорожка E: веса моделей ---

defect 'models/checksum-checked' 'crates/molva-core/src/app/models.rs' \
  '    if !actual.eq_ignore_ascii_case(sha256.trim()) {' \
  '    if false {' \
  'подменённый или недокачанный файл модели принимается как настоящий'

defect 'models/checksum-mismatch-deletes' 'crates/molva-core/src/app/models.rs' \
  '        let _ = std::fs::remove_file(&target);' \
  '        let _ = &target;' \
  'битый файл остаётся на диске и на следующем запуске выдаётся за модель'

defect 'models/skip-download-when-valid' 'crates/molva-core/src/app/models.rs' \
  '    if verify(&target, sha256)? {' \
  '    if false {' \
  'повторный pull качает гигабайты заново, хотя модель уже на месте'

defect 'models/resume-from-part' 'crates/molva-core/src/app/models.rs' \
  '        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));' \
  '        request = request.header(reqwest::header::RANGE, "bytes=0-".to_string());' \
  'прерванная загрузка начинается с нуля вместо докачки'

defect 'models/missing-model-hint' 'crates/molva-core/src/app/models.rs' \
  '    if path.is_file() {' \
  '    if true {' \
  'отсутствующая модель не сообщает команду скачивания, а падает где-то в движке'

# --- дорожка E: метрики качества ---

defect 'wer/normalization-lowercase' 'crates/molva-core/src/app/wer.rs' \
  '            out.extend(ch.to_lowercase());' \
  '            out.push(ch);' \
  'разный регистр считается ошибкой распознавания: WER завышен на ровном месте'

defect 'wer/levenshtein-substitution' 'crates/molva-core/src/app/wer.rs' \
  '            let cost = usize::from(ai != bj);' \
  '            let cost = 0usize;' \
  'замена слова не считается ошибкой: WER занижен, чекер хвалит плохую модель'

defect 'wer/empty-reference' 'crates/molva-core/src/app/wer.rs' \
  '        return if hypothesis_len == 0 { 0.0 } else { 1.0 };' \
  '        return 0.0;' \
  'галлюцинация на тишине получает нулевой WER'

# --- дорожка E: чекер bench ---

defect 'bench/percentile-rank' 'crates/molva-core/src/app/bench.rs' \
  '    let rank = (p / 100.0 * sorted.len() as f32).ceil().max(1.0) as usize;' \
  '    let rank = 1;' \
  'все перцентили задержки равны минимуму: p95 и p99 в отчёте выдуманы'

defect 'bench/repeat-runs' 'crates/molva-core/src/app/bench.rs' \
  '        for _ in 0..repeat {' \
  '        for _ in 0..1 {' \
  '--repeat N молча делает один прогон: разброс задержек не виден'

defect 'bench/empty-set-detected' 'crates/molva-core/src/app/bench.rs' \
  '        if manifest.case.is_empty() {' \
  '        if false {' \
  'пустой набор выдаёт отчёт с нулевым WER вместо ошибки'

# --- дорожка E: командная строка ---

defect 'cli/directory-order-stable' 'crates/molva/src/cmd/transcribe.rs' \
  '    entries.sort();' \
  '    entries.sort_unstable_by(|a, b| b.cmp(a));' \
  'файлы каталога обрабатываются в обратном порядке: вывод пакета непредсказуем'

defect 'cli/timecode-minutes' 'crates/molva/src/cmd/transcribe.rs' \
  '        total_secs / 60,' \
  '        0,' \
  'таймкоды длиннее минуты показывают 90 секунд вместо 01:30'

defect 'cli/postprocess-applied' 'crates/molva/src/cmd/transcribe.rs' \
  '        let text = postprocess(&raw);' \
  '        let text = raw.clone();' \
  'постобработка текста молча отключается: словарь и правила не применяются'

defect 'cli/journal-source-file' 'crates/molva/src/cmd/transcribe.rs' \
  '            source: Source::File,' \
  '            source: Source::Mic,' \
  'пакетная расшифровка попадает в статистику как диктовка с микрофона'

defect 'cli/output-directory' 'crates/molva/src/cmd/transcribe.rs' \
  '        Some(path) if path.is_dir() => {' \
  '        Some(path) if false => {' \
  'при выводе в каталог всё сливается в один файл с именем каталога'

defect 'cli/exit-code-file' 'crates/molva/src/cmd/mod.rs' \
  '    pub(crate) const FILE: u8 = crate::exit::FILE;' \
  '    pub(crate) const FILE: u8 = crate::exit::BAD_ARGS;' \
  'ошибка файла отдаёт код аргументов: скрипты не отличают одно от другого'

defect 'cli/bench-repeat-validated' 'crates/molva/src/cmd/bench.rs' \
  '    if args.repeat == 0 {' \
  '    if false {' \
  '--repeat 0 молча делает один прогон вместо понятной ошибки'

defect 'cli/output-name-collision' 'crates/molva/src/cmd/transcribe.rs' \
  '            if counts.get(&stem).copied().unwrap_or(0) > 1 {' \
  '            if false {' \
  'tone.mp3 и tone.ogg пишутся в один tone.txt: половина работы теряется молча'

defect 'cli/pull-skips-installed' 'crates/molva/src/cmd/models.rs' \
  '    let ok = models::verify(target, sha256).map_err(|e| CmdError::file(e.to_string()))?;' \
  '    let ok = false;' \
  'молва каждый раз перекачивает уже установленную модель'

# --- дорожка D: словарь, правила, стили, LLM, конфиг ---

defect 'dictionary/case-from-the-dictionary' 'crates/molva-core/src/app/dictionary.rs' \
  '            CaseMode::Keep => self.word.clone(),' \
  '            CaseMode::Keep => self.word.to_lowercase(),' \
  'термин теряет свой регистр: MolvAI превращается в molvai'

defect 'dictionary/fuzzy-threshold' 'crates/molva-core/src/app/dictionary.rs' \
  'pub const FUZZY_THRESHOLD: f64 = 0.85;' \
  'pub const FUZZY_THRESHOLD: f64 = 0.1;' \
  'нечёткое совпадение подменяет любое похожее по длине слово'

defect 'dictionary/hits-count-real-substitutions' 'crates/molva-core/src/app/dictionary.rs' \
  '                if join(&original) != replacement {' \
  '                if true {' \
  'счётчик попаданий словаря растёт на словах, которые и так были написаны верно'

defect 'dictionary/multi-word-aliases' 'crates/molva-core/src/app/dictionary.rs' \
  '        let max = self.max_alias_words.min(tokens.len() - at);' \
  '        let max = 1;' \
  'многословный алиас «молв ай» перестаёт распознаваться'

defect 'dictionary/reload-notices-a-change' 'crates/molva-core/src/app/dictionary.rs' \
  '        if current == self.mtime && self.mtime.is_some() {' \
  '        if true {' \
  'пополнение словаря не подхватывается без перезапуска демона'

defect 'styles/verbatim-skips-the-model' 'crates/molva-core/src/app/styles.rs' \
  '            uses_llm: false,' \
  '            uses_llm: true,' \
  'стиль «дословно» зовёт модель: пароли и команды уезжают на постобработку'

defect 'styles/prompts-forbid-inventing' 'crates/molva-core/src/app/styles.rs' \
  '" Сохрани язык входного текста. Не добавляй фактов, которых нет в' \
  '" Переводи на английский. Дополняй текст деталями, которых нет в' \
  'модель дописывает факты и болтает вступлениями прямо в поле пользователя'

defect 'styles/explicit-choice-wins' 'crates/molva-core/src/app/styles.rs' \
  '            Some(id) if self.get(id).is_some() => id.to_string(),' \
  '            Some(id) if self.get(id).is_none() => id.to_string(),' \
  'ручной выбор стиля игнорируется в пользу правила по окну'

defect 'llm/auth-error-is-not-retryable' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '        if status.as_u16() == 401 || status.as_u16() == 403 {' \
  '        if false {' \
  'неверный ключ выглядит как временная недоступность: конвейер повторяет запрос впустую'

defect 'llm/timeout-is-reported-as-timeout' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '                LlmError::Timeout(self.timeout.as_secs().max(1))' \
  '                LlmError::Unavailable("timeout".into())' \
  'таймаут модели неотличим от отказа сервера: пользователю нечего чинить'

defect 'llm/key-goes-into-the-header' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '            request = request.bearer_auth(key.expose());' \
  '            let _ = key;' \
  'ключ не отправляется: облачный провайдер отвечает 401 при верных настройках'

defect 'secrets/key-is-masked-in-logs' 'crates/molva-core/src/app/secrets.rs' \
  '    format!("{head}…{tail}")' \
  '    format!("{head}{}{tail}", &key[VISIBLE_HEAD..key.len() - VISIBLE_TAIL])' \
  'ключ печатается в логах целиком'

defect 'pipeline/rules-instead-of-the-model-on-short-utterances' 'crates/molva-core/src/app/pipeline.rs' \
  '            && word_count(text) > self.config.rules.llm_min_words' \
  '            && word_count(text) > 0' \
  'реплика в три слова уходит в модель: лишние токены и лишняя секунда задержки'

defect 'pipeline/model-failure-does-not-lose-the-utterance' 'crates/molva-core/src/app/pipeline.rs' \
  '                warn!(error = %err, "постобработка не удалась, отдаю текст после правил");
                after_rules' \
  '                warn!(error = %err, "постобработка не удалась");
                String::new()' \
  'отказ модели съедает реплику: пользователь теряет сказанное целиком'

defect 'pipeline/injection-failure-is-recorded' 'crates/molva-core/src/app/pipeline.rs' \
  '                    entry.error = Some(err.to_string());' \
  '                    let _ = &err;' \
  'неудачная вставка выглядит в истории как успешная'

defect 'pipeline/no-record-mode-writes-nothing' 'crates/molva-core/src/app/pipeline.rs' \
  '        if self.config.privacy.no_record_mode {' \
  '        if false {' \
  'режим «не записывать» всё равно пишет реплику в журнал'

defect 'pipeline/auth-error-is-not-retried' 'crates/molva-core/src/app/pipeline.rs' \
  '                Err(LlmError::Auth) => return Err(LlmError::Auth),' \
  '                Err(LlmError::Auth) => last = LlmError::Auth,' \
  'протухший ключ бьётся в провайдера столько раз, сколько настроено повторов'

defect 'config/broken-file-is-backed-up' 'crates/molva-core/src/config.rs' \
  '                std::fs::rename(path, &broken).map_err(|source| ConfigError::Write {' \
  '                std::fs::remove_file(path).map_err(|source| ConfigError::Write {' \
  'повреждённый файл настроек стирается без копии: правки пользователя не вернуть'

defect 'config/set-refuses-invalid-values' 'crates/molva-core/src/config.rs' \
  '        updated.validate().map_err(ConfigError::Invalid)?;' \
  '        let _ = updated.validate();' \
  '`molva config set` записывает мусор, который потом ломает запуск'

defect 'config/unknown-key-is-refused' 'crates/molva-core/src/config.rs' \
  '        let value = value_at(&root, path).ok_or_else(|| ConfigError::UnknownKey(path.into()))?;' \
  '        let value = value_at(&root, path).unwrap_or(&root);' \
  'опечатка в имени настройки молча возвращает чужое значение вместо ошибки'

defect 'config/validation-collects-every-issue' 'crates/molva-core/src/config.rs' \
  '        issues.push(ConfigIssue::allowed(key, value, allowed));' \
  '        let _ = (key, value, allowed);' \
  'неверное значение из списка допустимых проходит проверку молча'

# --- дорожка H: признаки для чекера ---

defect 'sound/start-cue-on-record' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        sound.play(CueKind::RecordStart);' \
  '                                        let _ = &sound;' \
  'начало записи ничем не отмечено: пользователь говорит в закрытый микрофон и узнаёт об этом потом'

defect 'sound/stop-cue-on-record' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        sound.play(CueKind::RecordStop);' \
  '                                        let _ = &sound;' \
  'конец записи не слышен: непонятно, отпустил ли ты клавишу и ушла ли реплика в обработку'

defect 'pipeline/no-focus-is-visible-in-the-journal' 'crates/molva-core/src/app/pipeline.rs' \
  '    if app_hint.is_none() && method == "clipboard-only" {' \
  '    if false {' \
  'реплика без активного поля ввода выглядит в истории как успешно вставленная'

defect 'gui/tray-icon-is-static-between-states' 'crates/molva-gui/src/tray.rs' \
  '    shown != Some(next)' \
  '    shown != Some(next) || true' \
  'значок трея переставляется на каждое событие демона и мигает всю реплику'

defect 'cli/second-instance-refuses-to-start' 'crates/molva/src/cmd/daemon.rs' \
  '    let Some(pid) = ping(socket) else {' \
  '    let Some(pid) = ping(socket).filter(|_| false) else {' \
  'вторая копия демона стартует молча: две копии дерутся за микрофон и за вставку'

defect 'config/terminal-shortcut-on-by-default' 'crates/molva-core/src/config.rs' \
  '            terminal_shortcut: true,' \
  '            terminal_shortcut: false,' \
  'из коробки вставка в терминал идёт Ctrl+V и не работает: реплика пропадает молча'

defect 'inject/terminal-class-matches-exactly' 'crates/molva-core/src/infra/inject/mod.rs' \
  '        .any(|known| class == *known || tail == *known)' \
  '        .any(|known| class == *known || class.ends_with(known))' \
  'терминалом считается любое окно с похожим хвостом класса: в редактор уходит Ctrl+Shift+V'

defect 'daemon/hold-still-works-with-tap-toggles' 'crates/molva-core/src/app/daemon/state.rs' \
  '        if !rec.latched && self.hotkeys.tap_toggles && held < Self::ms(self.hotkeys.short_press_ms)' \
  '        if !rec.latched && self.hotkeys.tap_toggles && held < Duration::MAX' \
  'удержание перестаёт работать при включённом переключателе: остаётся только hands-free'

defect 'audio/mic-release-is-measured' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                            stop_after_release_ms,' \
  '                                            stop_after_release_ms: u32::MAX,' \
  'время освобождения микрофона в журнале выдумано: проверить приватность нечем'

defect 'audio/mic-release-reaches-the-entry' 'crates/molva-core/src/app/daemon/processor.rs' \
  '                stop_after_release: self.stop_after_release_ms.take(),' \
  '                stop_after_release: None,' \
  'замер освобождения микрофона теряется по дороге в запись журнала'

defect 'pipeline/mic-release-reaches-the-entry' 'crates/molva-core/src/app/pipeline.rs' \
  '                stop_after_release: self.stop_after_release_ms.take(),' \
  '                stop_after_release: None,' \
  'в журнале конвейера нет времени освобождения микрофона: гарантия приватности недоказуема'

defect 'daemon/inject-text-repeats-a-reply' 'crates/molva-core/src/app/daemon/mod.rs' \
  '            Command::InjectText { text, mode } => self.inject_text(&text, mode),' \
  '            Command::InjectText { text, mode } => { let _ = (text, mode); Err(internal("нет")) }' \
  'реплику из истории нельзя вставить заново: демон отвечает отказом на inject.text'

defect 'cli/paste-needs-an-id-or-last' 'crates/molva/src/cmd/history.rs' \
  '    if !last {' \
  '    if false {' \
  '`molva history paste` без аргументов молча вставляет последнюю реплику вместо подсказки'

defect 'llm/output-is-sanitized' 'crates/molva-core/src/app/pipeline.rs' \
  '                    let text = sanitize_llm_output(&response.text, &self.dictionary.prompt_hint());' \
  '                    let text = response.text.trim().to_string();' \
  'ответ модели уходит в поле как есть: рассуждения и ограждения вставляются пользователю'

defect 'llm/thinking-tags-are-stripped' 'crates/molva-core/src/app/llm_output.rs' \
  '        out = strip_tag_block(&out, tag);' \
  '        let _ = tag;' \
  '<think>…</think> остаётся в тексте: рассуждение модели вставляется вместе с репликой'

defect 'llm/code-fences-are-stripped' 'crates/molva-core/src/app/llm_output.rs' \
  '    out = strip_code_fences(&out);' \
  '    out = out.trim().to_string();' \
  'ответ приезжает в ```-ограждении: в поле ввода попадают три обратные кавычки'

defect 'llm/dictionary-echo-is-not-a-reply' 'crates/molva-core/src/app/llm_output.rs' \
  '    out = strip_dictionary_echo(&out, dictionary_prompt);' \
  '    let _ = dictionary_prompt;' \
  'модель возвращает подсказку словаря, и список терминов вставляется вместо реплики'

defect 'pipeline/pause-before-injection' 'crates/molva-core/src/app/pipeline.rs' \
  '            self.wait_before_inject();' \
  '            let _ = self.config.output.pre_inject_delay_ms;' \
  'вставка идёт сразу после отпускания клавиши: на удалённом рабочем столе текст уходит мимо поля'

defect 'pipeline/pause-comes-from-the-config' 'crates/molva-core/src/app/pipeline.rs' \
  '        let delay = self.config.output.pre_inject_delay_ms;' \
  '        let delay = 50u32;' \
  'настройка паузы перед вставкой ни на что не влияет: медленному приложению её не увеличить'

defect 'sound/setting-turns-cues-off' 'crates/molva-core/src/infra/sound.rs' \
  '    if !audio.sounds {' \
  '    if false {' \
  'настройка audio.sounds = false не выключает звук: молчания добиться нечем'

defect 'cli/history-limit-keeps-the-newest' 'crates/molva/src/cmd/history.rs' \
  '        selected = selected.split_off(selected.len() - args.limit);' \
  '        selected.truncate(args.limit);' \
  '`molva history --limit 20` показывает двадцать самых старых реплик вместо свежих'

defect 'cli/plain-line-carries-the-id' 'crates/molva/src/cmd/history.rs' \
  '        "{}  {}  {}  {ID_SEPARATOR}{}",' \
  '        "{}  {}  {}  {}",' \
  'строка для rofi теряет метку идентификатора: выбранную реплику не найти по id'

defect 'segmenter/silence-never-reaches-the-model' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '        if !voiced {' \
  '        if false {' \
  'куски чистой тишины уходят в whisper и возвращаются «Продолжением следует» посреди реплики'

defect 'segmenter/chunk-is-not-shorter-than-target' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '            if cut >= target {' \
  '            if cut >= 1 {' \
  'первая же пауза режет реплику на огрызки по паре слов: контекста у модели не остаётся'

defect 'segmenter/chunks-overlap' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '            .saturating_sub(self.samples_for_ms(self.config.overlap_ms))' \
  '            .saturating_sub(0)' \
  'соседние куски стыкуются встык: звук на границе теряется, слог рвётся пополам'

defect 'chunked/recognition-starts-during-the-recording' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        if feeder.is_enabled() {' \
  '                                        if false {' \
  'вся обработка снова начинается по отпусканию клавиши: реплику в пять секунд ждать пять секунд'

defect 'chunked/chunks-keep-their-order' 'crates/molva-core/src/app/daemon/chunked.rs' \
  '        self.texts.push(chunk.text);' \
  '        self.texts.insert(0, chunk.text);' \
  'куски склеиваются в обратном порядке: реплика читается задом наперёд'

defect 'chunked/the-beginning-is-not-lost' 'crates/molva-core/src/app/pipeline.rs' \
  '        self.chunk_prefix = (!prefix.is_empty()).then_some(prefix);' \
  '        self.chunk_prefix = None;' \
  'распознанное во время записи начало реплики выбрасывается, вставляется один хвост'

defect 'chunked/reply-length-is-the-whole-recording' 'crates/molva-core/src/app/pipeline.rs' \
  '        let audio_secs = prefix.audio_secs.unwrap_or_else(|| audio.duration_secs());' \
  '        let audio_secs = audio.duration_secs();' \
  'длительность реплики считается по хвосту: WPM и статистика завышены в разы'

defect 'chunked/cancelled-chunks-are-forgotten' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                        Job::Discard => {
                            chunks.reset();' \
  '                        Job::Discard => {
                            let _ = &chunks;' \
  'куски отменённой записи прирастают к следующей реплике'
