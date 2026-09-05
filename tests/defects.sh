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

defect 'config/missing-file-is-default' 'crates/molva-core/src/config.rs' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ConfigError::Read { path: path.to_path_buf(), source: e }),' \
  'первый запуск без файла настроек падает вместо того, чтобы работать со значениями по умолчанию'

defect 'audio/trim-silence-threshold' 'crates/molva-core/src/app/audio/trim.rs' \
  '        .filter(|(_, chunk)| amplitude_to_db(rms(chunk)) >= threshold_db)' \
  '        .filter(|(_, chunk)| !chunk.is_empty())' \
  'тишина уходит в whisper целиком: модель галлюцинирует текст там, где никто не говорил'

defect 'audio/trim-keep-ms' 'crates/molva-core/src/app/audio/trim.rs' \
  '    let keep = (audio.sample_rate as u64 * keep_ms as u64 / 1000) as usize;' \
  '    let keep = 0;' \
  'запас keep_ms не оставлен: обрезка съедает первый слог реплики'

defect 'stt/language-retry' 'crates/molva-core/src/infra/stt/mod.rs' \
  '    if opts.language != LanguageHint::Auto {' \
  '    if true {' \
  'русская речь, принятая за украинскую, уходит в текст как есть: повтора с разрешённым языком нет'

defect 'stt/model-missing-message' 'crates/molva-core/src/infra/stt/whisper.rs' \
  '            if !self.model_path.exists() {' \
  '            if false {' \
  'вместо «скачайте модель» пользователь получает невнятную ошибку загрузки весов'

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
  '    pub const FILE: u8 = crate::exit::FILE;' \
  '    pub const FILE: u8 = crate::exit::BAD_ARGS;' \
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
