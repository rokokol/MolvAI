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

defect 'dictionary/case-from-the-dictionary' 'crates/molva-core/src/app/dictionary.rs' \
  '            CaseMode::Keep => self.word.clone(),' \
  '            CaseMode::Keep => self.word.to_lowercase(),' \
  'термин теряет свой регистр: MolvAI превращается в molvai'

defect 'dictionary/fuzzy-threshold' 'crates/molva-core/src/app/dictionary.rs' \
  'pub const FUZZY_THRESHOLD: f64 = 0.85;' \
  'pub const FUZZY_THRESHOLD: f64 = 0.1;' \
  'нечёткое совпадение подменяет любое похожее по длине слово'

defect 'dictionary/hits-count-real-substitutions' 'crates/molva-core/src/app/dictionary.rs' \
  '                    if join(&original) != replacement {' \
  '                    if true {' \
  'счётчик попаданий словаря растёт на словах, которые и так были написаны верно'

defect 'dictionary/multi-word-aliases' 'crates/molva-core/src/app/dictionary.rs' \
  '        let max = self.max_alias_words.min(tokens.len() - at);' \
  '        let max = 1;' \
  'многословный алиас «молв ай» перестаёт распознаваться'

defect 'dictionary/reload-notices-a-change' 'crates/molva-core/src/app/dictionary.rs' \
  '        if current == self.mtime && self.mtime.is_some() {' \
  '        if true {' \
  'пополнение словаря не подхватывается без перезапуска демона'
