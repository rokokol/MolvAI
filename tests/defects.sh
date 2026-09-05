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
