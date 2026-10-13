#!/usr/bin/env bash
# The list of defects for `scripts/t.sh falsify` (`just falsify`)
#
#   defect NAME FILE FIND REPLACE CONSEQUENCE
#
# Each entry disables one guarantee without breaking the compilation, and the suite must notice it.
# An entry is added together with the guarantee that it checks. FIND occurs in FILE exactly
# once, otherwise the harness reports `stale` — the list is behind the code

defect 'audio/mono-downmix-averages' 'crates/molva-core/src/domain/audio.rs' \
  '            sum / channels as f32' \
  '            sum' \
  'stereo is mixed down to mono with doubled volume: clipping and false signal levels'

defect 'audio/resample-changes-rate' 'crates/molva-core/src/domain/audio.rs' \
  '    if audio.sample_rate == target_rate || audio.samples.is_empty() || audio.sample_rate == 0 {' \
  '    if true {' \
  '48 kHz audio goes to whisper as 16 kHz: the speech is three times faster, and recognition falls apart'

defect 'entry/wpm-short-audio' 'crates/molva-core/src/domain/entry.rs' \
  '        if audio_secs < MIN_AUDIO_SECS_FOR_WPM {' \
  '        if audio_secs < 0.0 {' \
  'a phrase of half a second gives an absurd WPM and becomes a personal record'

defect 'entry/privacy-keeps-text' 'crates/molva-core/src/domain/entry.rs' \
  '        self.text_final = None;' \
  '        self.text_final = self.text_final.take();' \
  'in privacy mode the final text still goes into the journal'

defect 'text/word-count-punctuation' 'crates/molva-core/src/domain/text.rs' \
  '        .filter(|token| token.chars().any(char::is_alphanumeric))' \
  '        .filter(|_token| true)' \
  'dashes and punctuation marks count as words: WPM and word counters are too high'

defect 'inject/auto-threshold' 'crates/molva-core/src/domain/inject.rs' \
  '            OutputMode::Auto if text.chars().count() <= auto_type_max_chars => OutputMode::Type,' \
  '            OutputMode::Auto if text.len() <= auto_type_max_chars => OutputMode::Type,' \
  'the auto threshold counts bytes, not characters: Cyrillic goes to paste twice as early as configured'

defect 'daemon/min-hold-discards-audio' 'crates/molva-core/src/app/daemon/state.rs' \
  '        if !rec.latched && held < Self::ms(self.hotkeys.min_hold_ms) {' \
  '        if !rec.latched && held < Duration::ZERO {' \
  'an accidental touch of the key starts recognition of half a second of silence and pastes garbage'

defect 'daemon/second-start-is-busy' 'crates/molva-core/src/app/daemon/state.rs' \
  '                Outcome::busy("a recording is already running")' \
  '                Outcome::nothing()' \
  'a second press during a recording silently does nothing: the user does not understand that the daemon is busy'

defect 'daemon/toggle-stops-recording' 'crates/molva-core/src/app/daemon/state.rs' \
  '    fn toggle_off(&mut self, at: Instant) -> Outcome {' \
  '    fn toggle_off(&mut self, at: Instant) -> Outcome { self.last_press = Some(at); return Outcome::nothing();' \
  'hands-free cannot be turned off: toggle starts the recording and never stops it'

defect 'daemon/journal-records-every-reply' 'crates/molva-core/src/app/daemon/processor.rs' \
  '        self.journal.append(&stored)?;' \
  '        let _ = &stored;' \
  'phrases do not reach the journal: there is no history and no statistics, though everything "works"'

defect 'inject/chain-falls-back-to-clipboard' 'crates/molva-core/src/infra/inject/chain.rs' \
  '        let mut report = self.fallback.inject(text, OutputMode::Clipboard)?;' \
  '        let mut report = InjectReport::default();' \
  'when every paste method fails, the phrase is lost instead of going to the clipboard'

defect 'inject/terminal-needs-shift' 'crates/molva-core/src/infra/inject/chain.rs' \
  '        let terminal = class.is_some_and(is_terminal_class);' \
  '        let terminal = false;' \
  'in a terminal, Ctrl+V is sent, and it pastes nothing there: the phrase silently disappears'

defect 'daemon/window-reaches-injector' 'crates/molva-core/src/app/daemon/processor.rs' \
  '            self.injector.set_window(app_hint);' \
  '            self.injector.set_window(None);' \
  'the paste method does not know where it pastes: in terminals and in a browser it is equally wrong'

defect 'inject/clipboard-is-restored' 'crates/molva-core/src/infra/inject/clipboard.rs' \
  '        self.backend.restore(&saved)' \
  '        Ok(())' \
  'a link that the user copied is overwritten for good by the text of the phrase'

defect 'ipc/unknown-command-is-refused' 'crates/molva-core/src/infra/ipc/transport.rs' \
  '                    &Response::err(0, ErrorCode::BadRequest, err.to_string(), None),' \
  '                    &Response::ok(0, Value::Null),' \
  'a broken request gets "ok": the client thinks the command is done, but the daemon did not understand it'

defect 'ipc/stale-socket-is-removed' 'crates/molva-core/src/infra/ipc/transport.rs' \
  '        if path_is_stale(path) {' \
  '        if false {' \
  'after a crash the daemon does not start again: the socket file remained, and the address is taken'

defect 'platform/hyprland-is-detected' 'crates/molva-core/src/infra/platform.rs' \
  '    if env.hyprland_signature.is_some() {' \
  '    if false {' \
  'on Hyprland a wrong paste chain is chosen: hyprctl is not tried at all'

defect 'config/missing-file-is-default' 'crates/molva-core/src/config.rs' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),' \
  '            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ConfigError::Read { path: path.to_path_buf(), source: e }),' \
  'the first start without a settings file fails instead of working with the default values'

defect 'audio/trim-silence-threshold' 'crates/molva-core/src/app/audio/trim.rs' \
  '        .filter(|(_, chunk)| amplitude_to_db(rms(chunk)) >= threshold_db)' \
  '        .filter(|(_, chunk)| !chunk.is_empty())' \
  'all the silence goes to whisper: the model hallucinates text where nobody spoke'

defect 'audio/trim-keep-ms' 'crates/molva-core/src/app/audio/trim.rs' \
  '    let keep = (u64::from(audio.sample_rate) * u64::from(keep_ms) / 1000) as usize;' \
  '    let keep = 0;' \
  'the keep_ms margin is not kept: the trim eats the first syllable of the phrase'

defect 'stt/language-retry' 'crates/molva-core/src/infra/stt/mod.rs' \
  '    if options.language != LanguageHint::Auto {' \
  '    if true {' \
  'Russian speech taken for Ukrainian goes into the text as it is: there is no retry with the allowed language'

defect 'stt/model-missing-message' 'crates/molva-core/src/infra/stt/whisper.rs' \
  '            if !self.model_path.exists() {' \
  '            if false {' \
  'instead of "download the model", the user gets an unclear error about loading the weights'

defect 'stt/silence-gate' 'crates/molva-core/src/infra/stt/mod.rs' \
  '        .is_some_and(|p| p >= no_speech_threshold)' \
  '        .is_some_and(|p| p >= 1.1)' \
  'on silence whisper hallucinates, and the invented text is pasted into the active field'

defect 'audio/gain-clamp' 'crates/molva-core/src/infra/audio/cpal_source.rs' \
  '        *sample = (*sample * gain).clamp(-1.0, 1.0);' \
  '        *sample = *sample * gain;' \
  'the input gain pushes the signal out of range: clipping breaks recognition'

defect 'journal/privacy-strips-text' 'crates/molva-core/src/app/journal.rs' \
  '            owned = entry.clone().without_text();' \
  '            owned = entry.clone();' \
  'in privacy mode the texts of phrases still reach the journal file'

defect 'journal/corrupt-line-quarantined' 'crates/molva-core/src/app/journal.rs' \
  '                    broken.push(line.to_string());' \
  '                    let _ = &broken;' \
  'a broken line is silently lost: the user does not learn that a part of the history is unreadable'

defect 'journal/rotation-keeps-newest' 'crates/molva-core/src/app/journal.rs' \
  '            kept = &kept[kept.len() - max_entries as usize..];' \
  '            kept = &kept[..max_entries as usize];' \
  'the rotation keeps the oldest phrases and throws away the fresh ones'

defect 'journal/owner-only-permissions' 'crates/molva-core/src/app/journal.rs' \
  '        perms.set_mode(0o600);' \
  '        perms.set_mode(0o644);' \
  'any user of the system can read the journal with the texts of phrases'

defect 'stats/average-weighted-by-time' 'crates/molva-core/src/app/stats.rs' \
  '    Some(words as f32 / secs * 60.0)' \
  '    Some(entries.iter().filter_map(|e| Entry::wpm_for(e.words, e.audio_secs)).sum::<f32>() / entries.len() as f32)' \
  'the average speed is averaged over phrases: one short phrase outweighs an hour of dictation'

defect 'stats/record-needs-a-real-utterance' 'crates/molva-core/src/app/stats.rs' \
  '        if entry.audio_secs < RECORD_MIN_AUDIO_SECS || entry.words < RECORD_MIN_WORDS {' \
  '        if false {' \
  'a phrase of one and a half seconds with an absurd WPM becomes the personal record'

defect 'stats/streak-breaks-on-a-gap' 'crates/molva-core/src/app/stats.rs' \
  '    while days.contains(&cursor) {' \
  '    while days.iter().any(|day| *day <= cursor) {' \
  'the streak of days in a row does not notice the missed days and grows up to the first entry in the history'

defect 'stats/streak-starts-from-yesterday' 'crates/molva-core/src/app/stats.rs' \
  '        let yesterday = today - Duration::days(1);' \
  '        let yesterday = today - Duration::days(3);' \
  'the streak resets in the morning, before the first phrase of the new day'

defect 'stats/saved-minutes-subtract-dictation' 'crates/molva-core/src/app/stats.rs' \
  '        .map(|e| e.words as f32 / baseline - e.audio_secs / 60.0)' \
  '        .map(|e| e.words as f32 / baseline)' \
  'the saved time does not subtract the time of the dictation itself and is twice too high'

defect 'stats/session-splits-on-a-pause' 'crates/molva-core/src/app/stats.rs' \
  '                    || entry.ts - prev.ts > Duration::minutes(SESSION_GAP_MINUTES)' \
  '                    || false' \
  'a break of one hour does not split the session: the average speed per session is blurred by idle time'

defect 'stats/reset-marker-hides-old-entries' 'crates/molva-core/src/app/stats.rs' \
  '        Some(at) => entries.iter().filter(|e| e.ts >= at).cloned().collect(),' \
  '        Some(_) => entries.to_vec(),' \
  'the statistics reset resets nothing: old phrases keep counting'

defect 'rules/spoken-full-stop' 'crates/molva-core/src/app/rules.rs' \
  '    ("точка", "."),' \
  '    ("точка", ""),' \
  'the dictated word "точка" disappears instead of becoming a punctuation mark'

defect 'rules/point-of-view-is-not-a-command' 'crates/molva-core/src/app/rules.rs' \
  '    if normalize(&tokens[index]) != "точка" && normalize(&tokens[index]) != "точки" {' \
  '    if true {' \
  '"моя точка зрения" turns into "моя. зрения"'

defect 'rules/intensifier-repeat-survives' 'crates/molva-core/src/app/rules.rs' \
  '"очень", "чуть", "еле", "very"' \
  '' \
  '"очень очень важно" loses the emphasis: the removal of repeats cuts the meaning'

defect 'rules/fillers-never-empty-the-text' 'crates/molva-core/src/app/rules.rs' \
  '            .all(|token| !token.chars().any(char::is_alphanumeric))' \
  '            .all(|_token| false)' \
  'a phrase made only of fillers turns into an empty string: what was said is lost'

defect 'rules/numerals-need-a-falling-rank' 'crates/molva-core/src/app/rules.rs' \
  '            if found.class >= last_class {' \
  '            if false {' \
  '"три четыре" is added up to "7": a list of numbers breaks'

defect 'rules/ordinals-stay-in-words' 'crates/molva-core/src/app/rules.rs' \
  '    if followed_by_ordinal(tokens, at + len) {' \
  '    if false {' \
  '"две тысячи двадцать шестого года" turns into "2020 шестого года"'

defect 'rules/non-breaking-space-in-units' 'crates/molva-core/src/app/rules.rs' \
  '            out.push(format!("{}\u{a0}{}", tokens[index], tokens[index + 1]));' \
  '            out.push(format!("{} {}", tokens[index], tokens[index + 1]));' \
  '"5 кг" wraps across lines: the number is separated from the unit of measure'

defect 'rules/dot-inside-a-word-is-not-a-sentence-end' 'crates/molva-core/src/app/rules.rs' \
  '        None => true,' \
  '        None | Some(_) => true,' \
  '`whisper.cpp` turns into `whisper.Cpp`, and `example.com` into `example.Com`'

defect 'rules/no-space-inside-a-domain' 'crates/molva-core/src/app/rules.rs' \
  "        if ch == '.' && previous.is_alphanumeric() && !next.is_uppercase() {" \
  '        if false {' \
  'a mail address is split by a space: `ivan@example. com` is no longer an address'

defect 'rules/email-is-glued-back' 'crates/molva-core/src/app/rules.rs' \
  '                if is_address_part(&left) && is_address_part(&right) {' \
  '                if false {' \
  'a dictated address stays scattered in pieces around the at sign'

defect 'rules/no-space-before-punctuation' 'crates/molva-core/src/app/rules.rs' \
  "                    '.' | ',' | ';' | ':' | '!' | '?' | ')' | '»' | '…' | '\\u{201d}'" \
  "                    '\\u{0}'" \
  'a space remains before a period and a comma: the text looks like machine output'

# --- track E: file decoding ---

defect 'decode/downmix-to-mono' 'crates/molva-core/src/infra/audio/decode.rs' \
  '                mono.extend_from_slice(&downmix_to_mono(buf.samples(), channels));' \
  '                mono.extend_from_slice(buf.samples());' \
  'a stereo file goes to whisper as interleaved channels: twice as long and sounds like mush'

defect 'decode/empty-file-rejected' 'crates/molva-core/src/infra/audio/decode.rs' \
  '    if meta.len() == 0 {' \
  '    if false {' \
  'an empty file gives a format parsing error instead of a clear "0 bytes"'

defect 'decode/native-sample-rate' 'crates/molva-core/src/infra/audio/decode.rs' \
  '    Ok(PcmAudio::new(mono, sample_rate))' \
  '    Ok(PcmAudio::new(mono, 16_000))' \
  'the rate of the file is replaced by 16 kHz without resampling: the duration and the speed of speech are wrong'

# --- track E: model weights ---

defect 'models/checksum-checked' 'crates/molva-core/src/app/models.rs' \
  '    if !actual.eq_ignore_ascii_case(sha256.trim()) {' \
  '    if false {' \
  'a replaced or partly downloaded model file is accepted as genuine'

defect 'models/checksum-mismatch-deletes' 'crates/molva-core/src/app/models.rs' \
  '        let _ = std::fs::remove_file(&target);' \
  '        let _ = &target;' \
  'a broken file stays on the disk and is passed off as a model at the next start'

defect 'models/skip-download-when-valid' 'crates/molva-core/src/app/models.rs' \
  '    if verify(&target, sha256)? {' \
  '    if false {' \
  'a repeated pull downloads gigabytes again, though the model is already in place'

defect 'models/resume-from-part' 'crates/molva-core/src/app/models.rs' \
  '        request = request.header(reqwest::header::RANGE, format!("bytes={already}-"));' \
  '        request = request.header(reqwest::header::RANGE, "bytes=0-".to_string());' \
  'an interrupted download starts from zero instead of resuming'

defect 'models/missing-model-hint' 'crates/molva-core/src/app/models.rs' \
  '    if path.is_file() {' \
  '    if true {' \
  'a missing model does not report the download command, but fails somewhere in the engine'

defect 'models/stale-part-is-not-a-download' 'crates/molva-core/src/app/models.rs' \
  '    if idle > ACTIVE_DOWNLOAD_WINDOW {' \
  '    if false {' \
  'the remnant of an interrupted download looks in the window like an endless download stuck at one percent'

defect 'models/downloader-writes-the-watched-part' 'crates/molva-core/src/app/models.rs' \
  '    let part = partial_path(directory, file_name);' \
  '    let part = directory.join(format!("{file_name}.tmp"));' \
  'the downloader writes to a file other than the one the window watches: the model download is not visible'

defect 'models/unknown-model-has-no-download' 'crates/molva-core/src/app/models.rs' \
  '    let info = find(name).ok()?;' \
  '    let info = find(name).unwrap_or(&CATALOG[0]);' \
  'a model name that is not in the catalog shows the download of another model'

defect 'models/mirror-joined-by-one-slash' 'crates/molva-core/src/app/models.rs' \
  "    format!(\"{}/{}\", mirror.trim_end_matches('/'), info.file_name)" \
  '    format!("{}/{}", mirror, info.file_name)' \
  'a mirror with a slash at the end gives an address with two, and some servers answer 404'

# --- track E: quality metrics ---

defect 'wer/normalization-lowercase' 'crates/molva-core/src/app/wer.rs' \
  '            out.extend(ch.to_lowercase());' \
  '            out.push(ch);' \
  'a different letter case counts as a recognition error: WER is too high for no reason'

defect 'wer/levenshtein-substitution' 'crates/molva-core/src/app/wer.rs' \
  '            let cost = usize::from(ai != bj);' \
  '            let cost = 0usize;' \
  'a word substitution does not count as an error: WER is too low, and the checker praises a bad model'

defect 'wer/empty-reference' 'crates/molva-core/src/app/wer.rs' \
  '        return if hypothesis_len == 0 { 0.0 } else { 1.0 };' \
  '        return 0.0;' \
  'a hallucination on silence gets a zero WER'

# --- track E: the bench checker ---

defect 'bench/percentile-rank' 'crates/molva-core/src/app/bench.rs' \
  '    let rank = (p / 100.0 * sorted.len() as f32).ceil().max(1.0) as usize;' \
  '    let rank = 1;' \
  'all latency percentiles equal the minimum: p95 and p99 in the report are invented'

defect 'bench/repeat-runs' 'crates/molva-core/src/app/bench.rs' \
  '        for _ in 0..repeat {' \
  '        for _ in 0..1 {' \
  '--repeat N silently makes one run: the spread of latencies is not visible'

defect 'bench/empty-set-detected' 'crates/molva-core/src/app/bench.rs' \
  '        if manifest.case.is_empty() {' \
  '        if false {' \
  'an empty set gives a report with a zero WER instead of an error'

# --- track E: the command line ---

defect 'cli/directory-order-stable' 'crates/molva/src/cmd/transcribe.rs' \
  '    entries.sort();' \
  '    entries.sort_unstable_by(|a, b| b.cmp(a));' \
  'the files of a directory are processed in reverse order: the output of a batch is unpredictable'

defect 'cli/timecode-minutes' 'crates/molva/src/cmd/transcribe.rs' \
  '        total_secs / 60,' \
  '        0,' \
  'timecodes longer than a minute show 90 seconds instead of 01:30'

defect 'cli/postprocess-applied' 'crates/molva/src/cmd/transcribe.rs' \
  '        let processed = postprocess(&raw, &rules_language);' \
  '        let processed = Postprocessed { text: raw.clone(), dict_hits: 0, llm: None };' \
  'the post-processing of the text is silently disabled: the dictionary and the rules are not applied'

defect 'cli/journal-source-file' 'crates/molva/src/cmd/transcribe.rs' \
  '            source: Source::File,' \
  '            source: Source::Mic,' \
  'a batch transcript enters the statistics as a dictation from the microphone'

defect 'cli/output-directory' 'crates/molva/src/cmd/transcribe.rs' \
  '        Some(path) if path.is_dir() => {' \
  '        Some(path) if false => {' \
  'with output to a directory everything merges into one file named like the directory'

defect 'cli/exit-code-file' 'crates/molva/src/cmd/mod.rs' \
  '    pub(crate) const FILE: u8 = crate::exit::FILE;' \
  '    pub(crate) const FILE: u8 = crate::exit::BAD_ARGS;' \
  'a file error gives the arguments code: scripts cannot tell one from the other'

defect 'cli/bench-repeat-validated' 'crates/molva/src/cmd/bench.rs' \
  '    if args.repeat == 0 {' \
  '    if false {' \
  '--repeat 0 silently makes one run instead of a clear error'

defect 'cli/output-name-collision' 'crates/molva/src/cmd/transcribe.rs' \
  '            if counts.get(&stem).copied().unwrap_or(0) > 1 {' \
  '            if false {' \
  'tone.mp3 and tone.ogg are written to one tone.txt: half of the work is lost silently'

defect 'cli/pull-skips-installed' 'crates/molva/src/cmd/models.rs' \
  '    let ok = models::verify(target, sha256).map_err(|e| CmdError::file(e.to_string()))?;' \
  '    let ok = false;' \
  'molva downloads an already installed model again every time'

# --- track D: dictionary, rules, styles, LLM, config ---

defect 'dictionary/case-from-the-dictionary' 'crates/molva-core/src/app/dictionary.rs' \
  '            CaseMode::Keep => self.word.clone(),' \
  '            CaseMode::Keep => self.word.to_lowercase(),' \
  'a term loses its letter case: MolvAI turns into molvai'

defect 'dictionary/fuzzy-threshold' 'crates/molva-core/src/app/dictionary.rs' \
  'pub const FUZZY_THRESHOLD: f64 = 0.85;' \
  'pub const FUZZY_THRESHOLD: f64 = 0.1;' \
  'a fuzzy match replaces any word of similar length'

defect 'dictionary/hits-count-real-substitutions' 'crates/molva-core/src/app/dictionary.rs' \
  '                if join(&original) != replacement {' \
  '                if true {' \
  'the dictionary hit counter grows on words that were already spelled correctly'

defect 'dictionary/multi-word-aliases' 'crates/molva-core/src/app/dictionary.rs' \
  '        let max = self.max_alias_words.min(tokens.len() - at);' \
  '        let max = 1;' \
  'the multi-word alias "молв ай" is no longer recognized'

defect 'dictionary/reload-notices-a-change' 'crates/molva-core/src/app/dictionary.rs' \
  '        if current == self.fingerprint && self.fingerprint.is_some() {' \
  '        if true {' \
  'an addition to the dictionary is not picked up without a restart of the daemon'

defect 'hyprland/push-to-talk-ends-on-release' 'crates/molva-core/src/app/hotkeys/hyprland.rs' \
  '            release: true,' \
  '            release: false,' \
  'push-to-talk on Hyprland never stops: the release of the key does not end the phrase'

defect 'hyprland/cancel-key-reaches-the-application' 'crates/molva-core/src/app/hotkeys/hyprland.rs' \
  '            passes_through: true,' \
  '            passes_through: false,' \
  'with Escape as the cancel key, Escape stops working in every program under Hyprland'

defect 'hotkeys/keysym-comes-from-the-table' 'crates/molva-core/src/app/hotkeys/spec.rs' \
  '            return (*keysym).to_string();' \
  '            return (*keysym).to_lowercase();' \
  'the compositor binds name keys that do not exist, such as page_down, and Hyprland refuses the file'

defect 'styles/verbatim-skips-the-model' 'crates/molva-core/src/app/styles.rs' \
  '            uses_llm: false,' \
  '            uses_llm: true,' \
  'the verbatim style calls the model: passwords and commands go to post-processing'

defect 'styles/prompts-forbid-inventing' 'crates/molva-core/src/app/styles.rs' \
  '" Сохрани язык входного текста. Не добавляй фактов, которых нет в' \
  '" Переводи на английский. Дополняй текст деталями, которых нет в' \
  'the model adds facts and chatters with introductions right into the field of the user'

defect 'styles/explicit-choice-wins' 'crates/molva-core/src/app/styles.rs' \
  '            Some(id) if self.get(id).is_some() => id.to_string(),' \
  '            Some(id) if self.get(id).is_none() => id.to_string(),' \
  'the manual choice of a style is ignored in favour of the rule for the window'

defect 'styles/prompt-follows-the-phrase-language' 'crates/molva-core/src/domain/text.rs' \
  '        self.system_prompts.get(lang).unwrap_or(&self.system_prompt)' \
  '        &self.system_prompt' \
  'a Russian phrase is edited by the English prompt that the styles were not tuned with'

defect 'pipeline/style-prompt-gets-the-phrase-language' 'crates/molva-core/src/app/pipeline.rs' \
  '                .prompt_for(entry.language.as_deref().unwrap_or_default())' \
  '                .prompt_for("")' \
  'every phrase gets the English style prompt, whatever its language'

defect 'command/prompt-gets-the-instruction-language' 'crates/molva-core/src/app/pipeline.rs' \
  '        let lang = entry.language.as_deref().unwrap_or_default();' \
  '        let lang = "";' \
  'a Russian instruction in command mode gets the English prompt and request'

defect 'command/prompt-from-the-settings-wins' 'crates/molva-core/src/app/command_mode.rs' \
  '    if !configured.trim().is_empty() {' \
  '    if false {' \
  'the command prompt that the user wrote in the settings is silently ignored'

defect 'config/old-command-prompt-is-migrated' 'crates/molva-core/src/config.rs' \
  '            self.command_mode.system_prompt.clear();' \
  '            self.command_mode.system_prompt.push_str("");' \
  'every user who ever saved the settings keeps the Russian command prompt for English instructions'

defect 'llm/auth-error-is-not-retryable' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '        if status.as_u16() == 401 || status.as_u16() == 403 {' \
  '        if false {' \
  'a wrong key looks like a temporary outage: the pipeline repeats the request in vain'

defect 'llm/timeout-is-reported-as-timeout' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '                LlmError::Timeout(self.timeout.as_secs().max(1))' \
  '                LlmError::Unavailable("timeout".into())' \
  'a model timeout cannot be told from a server refusal: the user has nothing to fix'

defect 'llm/key-goes-into-the-header' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '            request = request.bearer_auth(key.expose());' \
  '            let _ = key;' \
  'the key is not sent: the cloud provider answers 401 with correct settings'

defect 'secrets/key-is-masked-in-logs' 'crates/molva-core/src/app/secrets.rs' \
  '    format!("{head}…{tail}")' \
  '    format!("{head}{}{tail}", &key[VISIBLE_HEAD..key.len() - VISIBLE_TAIL])' \
  'the key is printed in the logs in full'

defect 'pipeline/rules-instead-of-the-model-on-short-utterances' 'crates/molva-core/src/app/pipeline.rs' \
  '            && word_count(text) > self.config.rules.llm_min_words' \
  '            && word_count(text) > 0' \
  'a phrase of three words goes to the model: extra tokens and an extra second of delay'

defect 'pipeline/model-failure-does-not-lose-the-utterance' 'crates/molva-core/src/app/pipeline.rs' \
  '                after_rules' \
  '                String::new()' \
  'a model failure eats the phrase: the user loses everything that was said'

defect 'pipeline/injection-failure-is-recorded' 'crates/molva-core/src/app/pipeline.rs' \
  '                    entry.error = Some(err.to_string());' \
  '                    let _ = &err;' \
  'a failed paste looks like a successful one in the history'

defect 'pipeline/no-record-mode-writes-nothing' 'crates/molva-core/src/app/pipeline.rs' \
  '        if self.config.privacy.no_record_mode {' \
  '        if false {' \
  'the "do not record" mode still writes the phrase to the journal'

defect 'pipeline/auth-error-is-not-retried' 'crates/molva-core/src/app/pipeline.rs' \
  '                Err(LlmError::Auth) => return Err(LlmError::Auth),' \
  '                Err(LlmError::Auth) => last = LlmError::Auth,' \
  'an expired key hits the provider as many times as retries are configured'

defect 'config/broken-file-is-backed-up' 'crates/molva-core/src/config.rs' \
  '                std::fs::rename(path, &broken).map_err(|source| ConfigError::Write {' \
  '                std::fs::remove_file(path).map_err(|source| ConfigError::Write {' \
  'a damaged settings file is erased without a copy: the edits of the user cannot be restored'

defect 'config/set-refuses-invalid-values' 'crates/molva-core/src/config.rs' \
  '        updated.validate().map_err(ConfigError::Invalid)?;' \
  '        let _ = updated.validate();' \
  '`molva config set` writes garbage that later breaks the start'

defect 'config/unknown-key-is-refused' 'crates/molva-core/src/config.rs' \
  '        let value = value_at(&root, path).ok_or_else(|| ConfigError::UnknownKey(path.into()))?;' \
  '        let value = value_at(&root, path).unwrap_or(&root);' \
  'a typo in a setting name silently returns another value instead of an error'

defect 'config/validation-collects-every-issue' 'crates/molva-core/src/config.rs' \
  '        issues.push(ConfigIssue::allowed(key, value, allowed));' \
  '        let _ = (key, value, allowed);' \
  'a wrong value from the list of allowed ones passes the check silently'

# --- track H: signals for the checker ---

defect 'sound/start-cue-on-record' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        sound.play(CueKind::RecordStart);' \
  '                                        let _ = &sound;' \
  'the start of the recording is not marked in any way: the user speaks into a closed microphone and learns about it later'

defect 'sound/stop-cue-on-record' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        sound.play(CueKind::RecordStop);' \
  '                                        let _ = &sound;' \
  'the end of the recording is not audible: it is unclear whether you released the key and whether the phrase went to processing'

defect 'pipeline/no-focus-is-visible-in-the-journal' 'crates/molva-core/src/app/pipeline.rs' \
  '    if app_hint.is_none() && method == "clipboard-only" {' \
  '    if false {' \
  'a phrase without an active input field looks in the history like a successfully pasted one'

defect 'gui/tray-icon-is-static-between-states' 'crates/molva-gui/src/tray.rs' \
  '    shown != Some(next)' \
  '    shown != Some(next) || true' \
  'the tray icon is set again on every daemon event and flickers for the whole phrase'

defect 'gui/crash-reason-is-the-last-line' 'crates/molva-gui/src/sidecar.rs' \
  '            last_line = lock(&self.stderr_tail).back().cloned();' \
  '            last_line = lock(&self.stderr_tail).front().cloned();' \
  'the window shows the first journal line of the crashed daemon instead of its fatal error'

defect 'gui/clean-exit-is-not-a-crash' 'crates/molva-gui/src/sidecar.rs' \
  '        if status.success() {' \
  '        if false {' \
  'a daemon that stopped normally looks crashed in the window'

defect 'gui/download-is-shown-while-daemon-is-down' 'crates/molva-gui/src/commands.rs' \
  '        download,' \
  '        download: None,' \
  'while the daemon downloads weights, the window writes "The daemon is not running" instead of the progress'

defect 'gui/core-rules-checked-before-saving' 'crates/molva-gui/src/commands.rs' \
  '        if let Some(issue) = issues.into_iter().next() {' \
  '        if let Some(issue) = issues.into_iter().next().filter(|_| false) {' \
  'the window saves settings that the daemon will later reject, for example a mirror address without a scheme'

defect 'cli/second-instance-refuses-to-start' 'crates/molva/src/cmd/daemon.rs' \
  '    let Some(pid) = ping(socket) else {' \
  '    let Some(pid) = ping(socket).filter(|_| false) else {' \
  'a second copy of the daemon starts silently: two copies fight for the microphone and for the paste'

defect 'config/terminal-shortcut-on-by-default' 'crates/molva-core/src/config.rs' \
  '            terminal_shortcut: true,' \
  '            terminal_shortcut: false,' \
  'out of the box the paste into a terminal uses Ctrl+V and does not work: the phrase disappears silently'

defect 'inject/terminal-class-matches-exactly' 'crates/molva-core/src/infra/inject/mod.rs' \
  '        .any(|known| class == *known || tail == *known)' \
  '        .any(|known| class == *known || class.ends_with(known))' \
  'any window with a similar class tail counts as a terminal: Ctrl+Shift+V goes to an editor'

defect 'daemon/hold-still-works-with-tap-toggles' 'crates/molva-core/src/app/daemon/state.rs' \
  '        if !rec.latched && self.hotkeys.tap_toggles && held < Self::ms(self.hotkeys.short_press_ms)' \
  '        if !rec.latched && self.hotkeys.tap_toggles && held < Duration::MAX' \
  'the key hold stops working when the toggle is on: only hands-free remains'

defect 'audio/mic-release-is-measured' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                            stop_after_release_ms,' \
  '                                            stop_after_release_ms: u32::MAX,' \
  'the microphone release time in the journal is invented: there is nothing to check the privacy with'

defect 'audio/mic-release-reaches-the-entry' 'crates/molva-core/src/app/daemon/processor.rs' \
  '                stop_after_release: self.stop_after_release_ms.take(),' \
  '                stop_after_release: None,' \
  'the measurement of the microphone release is lost on the way to the journal entry'

defect 'pipeline/mic-release-reaches-the-entry' 'crates/molva-core/src/app/pipeline.rs' \
  '                stop_after_release: self.stop_after_release_ms.take(),' \
  '                stop_after_release: None,' \
  'the pipeline journal has no microphone release time: the privacy guarantee cannot be proved'

defect 'daemon/inject-text-repeats-a-reply' 'crates/molva-core/src/app/daemon/mod.rs' \
  '            Command::InjectText { text, mode } => self.inject_text(&text, mode),' \
  '            Command::InjectText { text, mode } => { let _ = (text, mode); Err(internal("none")) }' \
  'a phrase from the history cannot be pasted again: the daemon answers inject.text with a refusal'

defect 'cli/paste-needs-an-id-or-last' 'crates/molva/src/cmd/history.rs' \
  '    if !last {' \
  '    if false {' \
  '`molva history paste` without arguments silently pastes the last phrase instead of a hint'

defect 'llm/output-is-sanitized' 'crates/molva-core/src/app/pipeline.rs' \
  '                    let text = sanitize_llm_output(&response.text, &self.dictionary.prompt_hint());' \
  '                    let text = response.text.trim().to_string();' \
  'the answer of the model goes into the field as it is: the reasoning and the fences are pasted to the user'

defect 'llm/thinking-tags-are-stripped' 'crates/molva-core/src/app/llm_output.rs' \
  '        out = strip_tag_block(&out, tag);' \
  '        let _ = tag;' \
  '<think>…</think> stays in the text: the reasoning of the model is pasted together with the phrase'

defect 'llm/code-fences-are-stripped' 'crates/molva-core/src/app/llm_output.rs' \
  '    out = strip_code_fences(&out);' \
  '    out = out.trim().to_string();' \
  'the answer arrives in a ``` fence: three backticks get into the input field'

defect 'llm/dictionary-echo-is-not-a-reply' 'crates/molva-core/src/app/llm_output.rs' \
  '    out = strip_dictionary_echo(&out, dictionary_prompt);' \
  '    let _ = dictionary_prompt;' \
  'the model returns the hint of the dictionary, and the list of terms is pasted instead of the phrase'

defect 'pipeline/pause-before-injection' 'crates/molva-core/src/app/pipeline.rs' \
  '            self.wait_before_inject();' \
  '            let _ = self.config.output.pre_inject_delay_ms;' \
  'the paste goes right after the key is released: on a remote desktop the text misses the field'

defect 'pipeline/pause-comes-from-the-config' 'crates/molva-core/src/app/pipeline.rs' \
  '        let delay = self.config.output.pre_inject_delay_ms;' \
  '        let delay = 50u32;' \
  'the setting of the pause before the paste affects nothing: it cannot be increased for a slow application'

defect 'sound/setting-turns-cues-off' 'crates/molva-core/src/infra/sound.rs' \
  '    if !audio.sounds {' \
  '    if false {' \
  'the setting audio.sounds = false does not turn the sound off: there is no way to get silence'

defect 'cli/history-limit-keeps-the-newest' 'crates/molva/src/cmd/history.rs' \
  '        selected = selected.split_off(selected.len() - args.limit);' \
  '        selected.truncate(args.limit);' \
  '`molva history --limit 20` shows the twenty oldest phrases instead of the fresh ones'

defect 'cli/plain-line-carries-the-id' 'crates/molva/src/cmd/history.rs' \
  '        "{}  {}  {}  {ID_SEPARATOR}{}",' \
  '        "{}  {}  {}  {}",' \
  'the line for rofi loses the identifier mark: the chosen phrase cannot be found by id'

defect 'segmenter/silence-never-reaches-the-model' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '        if !voiced {' \
  '        if false {' \
  'chunks of pure silence go to whisper and come back as "Продолжение следует" in the middle of a phrase'

defect 'segmenter/chunk-is-not-shorter-than-target' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '            if cut >= target {' \
  '            if cut >= 1 {' \
  'the first pause cuts the phrase into scraps of a couple of words: the model has no context left'

defect 'segmenter/chunks-overlap' 'crates/molva-core/src/app/audio/segmenter.rs' \
  '            .saturating_sub(self.samples_for_ms(self.config.overlap_ms))' \
  '            .saturating_sub(0)' \
  'neighbouring chunks join edge to edge: the sound at the border is lost, and a syllable is torn in half'

defect 'chunked/recognition-starts-during-the-recording' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                                        if feeder.is_enabled() {' \
  '                                        if false {' \
  'all the processing starts again on key release: a phrase of five seconds waits five seconds'

defect 'chunked/chunks-keep-their-order' 'crates/molva-core/src/app/daemon/chunked.rs' \
  '        self.texts.push(chunk.text);' \
  '        self.texts.insert(0, chunk.text);' \
  'chunks are joined in reverse order: the phrase reads backwards'

defect 'chunked/the-beginning-is-not-lost' 'crates/molva-core/src/app/pipeline.rs' \
  '        self.chunk_prefix = (!prefix.is_empty()).then_some(prefix);' \
  '        self.chunk_prefix = None;' \
  'the beginning of the phrase that was recognized during the recording is thrown away, and only the tail is pasted'

defect 'chunked/reply-length-is-the-whole-recording' 'crates/molva-core/src/app/pipeline.rs' \
  '        let audio_secs = prefix.audio_secs.unwrap_or_else(|| audio.duration_secs());' \
  '        let audio_secs = audio.duration_secs();' \
  'the duration of the phrase is counted by the tail: WPM and statistics are several times too high'

defect 'chunked/cancelled-chunks-are-forgotten' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                        Job::Discard => {
                            chunks.reset();' \
  '                        Job::Discard => {
                            let _ = &chunks;' \
  'the chunks of a cancelled recording attach to the next phrase'

defect 'transcribe/short-text-skips-the-model' 'crates/molva/src/cmd/transcribe.rs' \
  '        if !self.style.uses_llm || word_count(&after_rules) <= self.llm_min_words {' \
  '        if !self.style.uses_llm {' \
  'short phrases from files go to the model against rules.llm_min_words'

defect 'transcribe/empty-model-answer-keeps-the-rules-text' 'crates/molva/src/cmd/transcribe.rs' \
  '                if text.is_empty() {
                    // An empty answer is not a result' \
  '                if false {
                    // An empty answer is not a result' \
  'an empty answer of the model overwrites the transcript of the file'

defect 'transcribe/model-trace-reaches-the-journal' 'crates/molva/src/cmd/transcribe.rs' \
  '            llm_used: processed.llm.is_some(),' \
  '            llm_used: false,' \
  'the journal cannot tell the files that went through the model from the others: the token statistics are wrong'

defect 'llm/auto-effort-silences-local-reasoning' 'crates/molva-core/src/infra/llm/openai_compat.rs' \
  '        "auto" => is_local.then(|| "none".to_string()),' \
  '        "auto" => None,' \
  'a local reasoning model burns max_tokens on thinking and returns an empty text'

defect 'dictionary/reload-notices-a-same-mtime-change' 'crates/molva-core/src/app/dictionary.rs' \
  '        len: metadata.len(),' \
  '        len: 0,' \
  'an edit of the dictionary with the same timestamp stays unnoticed until the daemon restarts'

defect 'doctor/wrong-model-hash-is-not-ok' 'crates/molva/src/cmd/doctor.rs' \
  '        Ok(false) => Check::new(
            "whisper model",
            false,' \
  '        Ok(false) => Check::new(
            "whisper model",
            true,' \
  'doctor calls broken whisper weights sound'

defect 'doctor/unreachable-llm-is-not-ok' 'crates/molva/src/cmd/doctor.rs' \
  '        Err(error) => Check::new(
            "LLM",
            false,' \
  '        Err(error) => Check::new(
            "LLM",
            true,' \
  'doctor calls an unreachable model provider working'

defect 'daemon/hold-counts-from-the-open-microphone' 'crates/molva-core/src/app/daemon/state.rs' \
  '        if let Phase::Recording(rec) = &mut self.phase {
            rec.since = at;
        }' \
  '        if let Phase::Recording(rec) = &mut self.phase {
            let _ = (rec, at);
        }' \
  'a slow opening of the microphone turns a short tap into a key hold: the recording ends empty'

defect 'journal/encrypt-seals-the-final-text' 'crates/molva-core/src/app/journal.rs' \
  '            sealed.text_final = sealed
                .text_final
                .as_deref()
                .map(|text| cipher.encrypt(text));' \
  '            sealed.text_final = sealed.text_final.clone();' \
  'journal.encrypt = true, but the final text of the phrase lies in the file in the open'

defect 'journal/decrypt-hides-ciphertext-without-a-key' 'crates/molva-core/src/app/journal.rs' \
  '            self.cipher
                .as_ref()
                .and_then(|cipher| cipher.decrypt(&value))' \
  '            self.cipher
                .as_ref()
                .and_then(|cipher| cipher.decrypt(&value))
                .or(Some(value))' \
  'the history without a key shows ciphertext instead of an empty field, and with a key it shows ciphertext after a failure too'

defect 'daemon/config-reload-reaches-the-pipeline' 'crates/molva-core/src/app/daemon/mod.rs' \
  '                            let _ = work_tx.send(Job::Reload(config));' \
  '                            let _ = &config;' \
  'a style change from the tray and config reload do not reach the pipeline: the dictation goes on with the old style'

defect 'daemon/idle-waits-for-every-reply' 'crates/molva-core/src/app/daemon/state.rs' \
  '                if self.in_flight == 0 && matches!(self.phase, Phase::Processing(_)) {' \
  '                if matches!(self.phase, Phase::Processing(_)) {' \
  'the daemon shows "ready" while the second phrase is still being recognized'

defect 'secrets/keyring-preferred-over-env' 'crates/molva-core/src/app/secrets.rs' \
  '                    return Some(value);' \
  '                    let _ = value;' \
  'the key from the OS store is ignored: with api_key_source = keyring only the environment variable is read'

defect 'secrets/journal-key-is-reused' 'crates/molva-core/src/app/journal_crypto.rs' \
  '        return parse_key(text.trim(), name);' \
  '        let _ = text;' \
  'a new key is written to the store each time the journal opens: the phrases from yesterday are unreadable'

defect 'secrets/file-store-owner-only' 'crates/molva-core/src/app/secrets.rs' \
  '            options.mode(0o600);' \
  '            options.mode(0o644);' \
  'the file with the provider key can be read by any user of the machine'

defect 'secrets/doctor-probe-is-deleted' 'crates/molva/src/cmd/doctor.rs' \
  '        .and_then(|read| store.delete(probe).map(|()| read));' \
  '        .and_then(Ok);' \
  'each run of molva doctor leaves a probe entry in the OS store'

defect 'secrets/journal-key-must-be-hex' 'crates/molva/src/cmd/secret.rs' \
  '    if name == SecretName::Journal' \
  '    if false' \
  '"molva secret set journal" accepts anything, and the daemon then cannot open the journal'
