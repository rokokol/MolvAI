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
