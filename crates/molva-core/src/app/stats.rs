// SPDX-License-Identifier: MIT
//! Статистика по журналу реплик: сводка, ряды по дням, разбивка по приложениям, сессии.
//!
//! Все средние скорости взвешены по времени (`sum(words) / sum(audio_secs) * 60`), а не
//! усреднены по полю `wpm`: иначе одна короткая реплика перевешивает час диктовки.
//!
//! ## Границы дня
//!
//! Дни считаются по календарной дате UTC — той же, что стоит в поле `ts` каждой строки журнала.
//! Так сводка воспроизводима на любой машине и совпадает с тем, что лежит в файле; цена — в
//! часовых поясах далеко от UTC «сегодня» в статистике смещено относительно местного дня.
//!
//! ## Что за какой период
//!
//! - за всё время (после маркера сброса): `total_words`, `avg_wpm_all`, `record_wpm`,
//!   `record_at`, `streak_days`, `minutes_recorded`, `saved_minutes`;
//! - за сегодня: `words_today`, `avg_wpm_today`;
//! - за 7 дней: `avg_wpm_7d`;
//! - за запрошенное окно `range_days`: `series`, `by_app`, `latency_ms`, `tokens`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::config::StatsConfig;
use crate::domain::entry::Entry;

/// Реплики короче этого не претендуют на личный рекорд.
pub const RECORD_MIN_AUDIO_SECS: f32 = 3.0;
/// И короче этого — тоже.
pub const RECORD_MIN_WORDS: u32 = 5;
/// Разрыв, после которого статистика делит сессию, даже если `session_id` тот же.
pub const SESSION_GAP_MINUTES: i64 = 30;
/// Имя файла с маркером сброса статистики рядом с журналом.
pub const RESET_MARKER_FILE: &str = "stats-reset.json";

#[derive(Debug, Error)]
pub enum StatsError {
    #[error("не удалось прочитать маркер сброса {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("не удалось записать маркер сброса {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Средние задержки стадий за период, миллисекунды.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LatencySummary {
    pub stt: u32,
    /// `None`, если модель постобработки не вызывалась ни разу.
    pub llm: Option<u32>,
    /// `None`, если ни одна реплика не дошла до вставки.
    pub inject: Option<u32>,
    pub total: u32,
}

/// Суммарные токены модели постобработки за период.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenSummary {
    pub prompt: u64,
    pub completion: u64,
}

/// Один день ряда: основа графика скорости и спарклайна.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayStats {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub entries: u32,
    pub words: u64,
    pub audio_secs: f32,
    pub avg_wpm: Option<f32>,
    pub avg_latency_ms: u32,
}

/// Разбивка по приложениям за период.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStats {
    pub app: String,
    pub entries: u32,
    pub words: u64,
    pub avg_wpm: Option<f32>,
}

/// Сводка статистики. Форма JSON — контракт для GUI и `molva stats --json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSummary {
    pub total_words: u64,
    pub words_today: u64,
    pub avg_wpm_today: Option<f32>,
    pub avg_wpm_7d: Option<f32>,
    pub avg_wpm_all: Option<f32>,
    pub record_wpm: Option<f32>,
    pub record_at: Option<DateTime<Utc>>,
    pub streak_days: u32,
    pub minutes_recorded: f32,
    pub saved_minutes: f32,
    pub latency_ms: LatencySummary,
    pub tokens: TokenSummary,
    pub series: Vec<DayStats>,
    pub by_app: Vec<AppStats>,
}

/// Отрезок непрерывной работы: один `session_id` без пауз длиннее 30 минут.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub session_id: Uuid,
    pub started: DateTime<Utc>,
    pub ended: DateTime<Utc>,
    pub entries: u32,
    pub words: u64,
    pub audio_secs: f32,
    pub avg_wpm: Option<f32>,
}

/// Средняя скорость, взвешенная по времени: `sum(words) / sum(audio_secs) * 60`.
///
/// `None`, когда суммарной длительности нет — делить не на что.
pub fn weighted_wpm(entries: &[&Entry]) -> Option<f32> {
    let secs: f32 = entries.iter().map(|e| e.audio_secs).sum();
    if secs <= 0.0 {
        return None;
    }
    let words: u64 = entries.iter().map(|e| u64::from(e.words)).sum();
    Some(words as f32 / secs * 60.0)
}

/// Скорость реплики: пересчитывается из слов и длительности, а не берётся на веру из поля.
fn entry_wpm(entry: &Entry) -> Option<f32> {
    Entry::wpm_for(entry.words, entry.audio_secs)
}

fn day_key(ts: DateTime<Utc>) -> NaiveDate {
    ts.date_naive()
}

/// Убрать минус у нуля: сумма пустого итератора в Rust равна `-0.0`, а «-0.0 мин» в отчёте
/// выглядит как ошибка вычислений.
fn no_minus_zero(value: f32) -> f32 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

/// Сводка по журналу. `range_days` — окно рядов и разбивок; `0` — все дни, где что-то было.
pub fn summary(
    entries: &[Entry],
    now: DateTime<Utc>,
    cfg: &StatsConfig,
    range_days: u32,
) -> StatsSummary {
    let all: Vec<&Entry> = entries.iter().collect();
    let today = day_key(now);

    let today_entries: Vec<&Entry> = all
        .iter()
        .copied()
        .filter(|e| day_key(e.ts) == today)
        .collect();
    let week_start = today - Duration::days(6);
    let week_entries: Vec<&Entry> = all
        .iter()
        .copied()
        .filter(|e| day_key(e.ts) >= week_start && day_key(e.ts) <= today)
        .collect();

    let range_start = if range_days == 0 {
        None
    } else {
        Some(today - Duration::days(i64::from(range_days) - 1))
    };
    let range: Vec<&Entry> = all
        .iter()
        .copied()
        .filter(|e| {
            let day = day_key(e.ts);
            day <= today && range_start.is_none_or(|start| day >= start)
        })
        .collect();

    let (record_wpm, record_at) = personal_record(&all);
    let total_secs: f32 = all.iter().map(|e| e.audio_secs).sum();
    let total_words: u64 = all.iter().map(|e| u64::from(e.words)).sum();

    StatsSummary {
        total_words,
        words_today: today_entries.iter().map(|e| u64::from(e.words)).sum(),
        avg_wpm_today: weighted_wpm(&today_entries),
        avg_wpm_7d: weighted_wpm(&week_entries),
        avg_wpm_all: weighted_wpm(&all),
        record_wpm,
        record_at,
        streak_days: streak_days(&all, today),
        minutes_recorded: no_minus_zero(total_secs / 60.0),
        saved_minutes: no_minus_zero(saved_minutes(&all, cfg)),
        latency_ms: latency_summary(&range),
        tokens: token_summary(&range),
        series: series(&range, range_start, today),
        by_app: by_app(&range),
    }
}

/// То же, но с маркером сброса: записи раньше маркера в сводку не попадают.
pub fn summary_since_reset(
    entries: &[Entry],
    reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    cfg: &StatsConfig,
    range_days: u32,
) -> StatsSummary {
    let kept = entries_since_reset(entries, reset_at);
    summary(&kept, now, cfg, range_days)
}

/// Записи, попадающие в статистику после сброса. История при этом не теряется.
pub fn entries_since_reset(entries: &[Entry], reset_at: Option<DateTime<Utc>>) -> Vec<Entry> {
    match reset_at {
        None => entries.to_vec(),
        Some(at) => entries.iter().filter(|e| e.ts >= at).cloned().collect(),
    }
}

/// Личный рекорд: максимальная скорость среди достаточно длинных реплик.
pub fn personal_record(entries: &[&Entry]) -> (Option<f32>, Option<DateTime<Utc>>) {
    let mut best: Option<(f32, DateTime<Utc>)> = None;
    for entry in entries {
        if entry.audio_secs < RECORD_MIN_AUDIO_SECS || entry.words < RECORD_MIN_WORDS {
            continue;
        }
        let Some(wpm) = entry_wpm(entry) else {
            continue;
        };
        if best.is_none_or(|(top, _)| wpm > top) {
            best = Some((wpm, entry.ts));
        }
    }
    match best {
        Some((wpm, at)) => (Some(wpm), Some(at)),
        None => (None, None),
    }
}

/// Дни подряд с диктовкой, считая назад от сегодня.
///
/// Если сегодня записей ещё нет, серия отсчитывается от вчера: день не считается пропущенным,
/// пока он не закончился.
pub fn streak_days(entries: &[&Entry], today: NaiveDate) -> u32 {
    if entries.is_empty() {
        return 0;
    }
    let days: std::collections::BTreeSet<NaiveDate> =
        entries.iter().map(|e| day_key(e.ts)).collect();
    let mut cursor = if days.contains(&today) {
        today
    } else {
        let yesterday = today - Duration::days(1);
        if days.contains(&yesterday) {
            yesterday
        } else {
            return 0;
        }
    };
    let mut streak = 0;
    while days.contains(&cursor) {
        streak += 1;
        cursor -= Duration::days(1);
    }
    streak
}

/// Сэкономленные минуты против набора руками: `words / baseline - audio_secs / 60`.
pub fn saved_minutes(entries: &[&Entry], cfg: &StatsConfig) -> f32 {
    if cfg.typing_baseline_wpm == 0 {
        return 0.0;
    }
    let baseline = cfg.typing_baseline_wpm as f32;
    entries
        .iter()
        .map(|e| e.words as f32 / baseline - e.audio_secs / 60.0)
        .sum()
}

/// Средние задержки стадий: арифметическое среднее по репликам, где стадия была.
pub fn latency_summary(entries: &[&Entry]) -> LatencySummary {
    fn mean<I: Iterator<Item = u32>>(values: I) -> Option<u32> {
        let mut sum = 0u64;
        let mut count = 0u64;
        for value in values {
            sum += u64::from(value);
            count += 1;
        }
        sum.checked_div(count).map(|mean| mean as u32)
    }

    LatencySummary {
        stt: mean(entries.iter().map(|e| e.latency_ms.stt)).unwrap_or(0),
        llm: mean(entries.iter().filter_map(|e| e.latency_ms.llm)),
        inject: mean(entries.iter().filter_map(|e| e.latency_ms.inject)),
        total: mean(entries.iter().map(|e| e.latency_ms.total)).unwrap_or(0),
    }
}

/// Суммарные токены: prompt и completion считаются раздельно (критерий S-05).
pub fn token_summary(entries: &[&Entry]) -> TokenSummary {
    let mut sum = TokenSummary::default();
    for entry in entries {
        if let Some(tokens) = &entry.tokens {
            sum.prompt += u64::from(tokens.prompt);
            sum.completion += u64::from(tokens.completion);
        }
    }
    sum
}

/// Ряд по дням. При заданном окне пустые дни тоже попадают в ряд — иначе график врёт о форме.
pub fn series(
    entries: &[&Entry],
    range_start: Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<DayStats> {
    let mut buckets: BTreeMap<NaiveDate, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        buckets.entry(day_key(entry.ts)).or_default().push(entry);
    }
    let days: Vec<NaiveDate> = match range_start {
        Some(start) => {
            let mut days = Vec::new();
            let mut cursor = start;
            while cursor <= today {
                days.push(cursor);
                cursor += Duration::days(1);
            }
            days
        }
        None => buckets.keys().copied().collect(),
    };
    days.into_iter()
        .map(|day| {
            let empty: Vec<&Entry> = Vec::new();
            let day_entries = buckets.get(&day).unwrap_or(&empty);
            DayStats {
                day: day.format("%Y-%m-%d").to_string(),
                entries: day_entries.len() as u32,
                words: day_entries.iter().map(|e| u64::from(e.words)).sum(),
                audio_secs: no_minus_zero(day_entries.iter().map(|e| e.audio_secs).sum()),
                avg_wpm: weighted_wpm(day_entries),
                avg_latency_ms: latency_summary(day_entries).total,
            }
        })
        .collect()
}

/// Разбивка по приложениям, по убыванию числа слов. Реплики без приложения идут как `unknown`.
pub fn by_app(entries: &[&Entry]) -> Vec<AppStats> {
    let mut buckets: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        let key = entry.app.clone().unwrap_or_else(|| "unknown".into());
        buckets.entry(key).or_default().push(entry);
    }
    let mut stats: Vec<AppStats> = buckets
        .into_iter()
        .map(|(app, group)| AppStats {
            app,
            entries: group.len() as u32,
            words: group.iter().map(|e| u64::from(e.words)).sum(),
            avg_wpm: weighted_wpm(&group),
        })
        .collect();
    stats.sort_by(|a, b| b.words.cmp(&a.words).then_with(|| a.app.cmp(&b.app)));
    stats
}

/// Сессии: группировка по `session_id` с дополнительным разрывом больше 30 минут.
pub fn sessions(entries: &[Entry]) -> Vec<Session> {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.ts);

    let mut sessions: Vec<Vec<&Entry>> = Vec::new();
    let mut current: Vec<&Entry> = Vec::new();
    for entry in sorted {
        let split = match current.last() {
            None => false,
            Some(prev) => {
                prev.session_id != entry.session_id
                    || entry.ts - prev.ts > Duration::minutes(SESSION_GAP_MINUTES)
            }
        };
        if split {
            sessions.push(std::mem::take(&mut current));
        }
        current.push(entry);
    }
    if !current.is_empty() {
        sessions.push(current);
    }

    sessions
        .into_iter()
        .map(|group| Session {
            session_id: group[0].session_id,
            started: group[0].ts,
            ended: group[group.len() - 1].ts,
            entries: group.len() as u32,
            words: group.iter().map(|e| u64::from(e.words)).sum(),
            audio_secs: group.iter().map(|e| e.audio_secs).sum(),
            avg_wpm: weighted_wpm(&group),
        })
        .collect()
}

/// CSV-выгрузка статистики по репликам.
pub fn export_csv(entries: &[Entry]) -> String {
    let mut out = String::from("ts,wpm,words,audio_secs,app,style,latency_total_ms\n");
    for entry in entries {
        let row = [
            entry.ts.to_rfc3339(),
            entry_wpm(entry)
                .map(|w| format!("{w:.1}"))
                .unwrap_or_default(),
            entry.words.to_string(),
            format!("{:.2}", entry.audio_secs),
            entry.app.clone().unwrap_or_default(),
            entry.style.clone(),
            entry.latency_ms.total.to_string(),
        ];
        let row: Vec<String> = row.iter().map(|f| super::journal::csv_field(f)).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

/// Путь к маркеру сброса рядом с журналом.
pub fn reset_marker_path(journal_path: &Path) -> PathBuf {
    journal_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(RESET_MARKER_FILE)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ResetMarker {
    reset_at: DateTime<Utc>,
}

/// Записать маркер сброса: статистика начинает считаться с этого момента, история цела.
pub fn write_reset_marker(journal_path: &Path, at: DateTime<Utc>) -> Result<(), StatsError> {
    let path = reset_marker_path(journal_path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|source| StatsError::Write {
                path: path.clone(),
                source,
            })?;
        }
    }
    let text = serde_json::to_string(&ResetMarker { reset_at: at })
        .unwrap_or_else(|_| format!("{{\"reset_at\":\"{}\"}}", at.to_rfc3339()));
    std::fs::write(&path, text).map_err(|source| StatsError::Write { path, source })
}

/// Прочитать маркер сброса. Нет файла или он повреждён — считаем, что сброса не было.
pub fn read_reset_marker(journal_path: &Path) -> Option<DateTime<Utc>> {
    let path = reset_marker_path(journal_path);
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<ResetMarker>(&text)
        .ok()
        .map(|marker| marker.reset_at)
}

/// Убрать маркер: статистика снова считается по всей истории.
pub fn clear_reset_marker(journal_path: &Path) -> Result<(), StatsError> {
    let path = reset_marker_path(journal_path);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(StatsError::Write { path, source }),
    }
}

/// Блоки спарклайна от пустого к полному.
const SPARK_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Спарклайн для ряда значений; шкала от нуля, чтобы столбики были сравнимы между собой.
pub fn sparkline(values: &[f32]) -> String {
    if values.is_empty() {
        return String::new();
    }
    let max = values.iter().copied().fold(0.0_f32, f32::max);
    if max <= 0.0 {
        return SPARK_BLOCKS[0].to_string().repeat(values.len());
    }
    values
        .iter()
        .map(|value| {
            let level = (value / max * (SPARK_BLOCKS.len() - 1) as f32).round() as usize;
            SPARK_BLOCKS[level.min(SPARK_BLOCKS.len() - 1)]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::journal::test_entry;
    use crate::domain::entry::Tokens;

    fn now_at(ts: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(ts)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn cfg() -> StatsConfig {
        StatsConfig::default()
    }

    #[test]
    fn average_speed_is_weighted_by_time_not_by_entry() {
        // 100 слов за 60 с и 1 слово за 6 с: арифметическое среднее дало бы 55 wpm.
        let entries = [
            test_entry("2026-09-05T10:00:00Z", 100, 60.0, "kitty"),
            test_entry("2026-09-05T10:05:00Z", 1, 6.0, "kitty"),
        ];
        let refs: Vec<&Entry> = entries.iter().collect();
        let wpm = weighted_wpm(&refs).unwrap();
        assert!((wpm - 91.8).abs() < 0.1, "{wpm}");
    }

    #[test]
    fn average_speed_without_audio_is_absent() {
        assert_eq!(weighted_wpm(&[]), None);
        let entry = test_entry("2026-09-05T10:00:00Z", 5, 0.0, "kitty");
        assert_eq!(weighted_wpm(&[&entry]), None);
    }

    #[test]
    fn today_is_bounded_by_the_utc_date() {
        let entries = vec![
            test_entry("2026-09-04T23:59:59Z", 7, 4.0, "kitty"),
            test_entry("2026-09-05T00:00:00Z", 11, 4.0, "kitty"),
            test_entry("2026-09-05T23:59:59Z", 13, 4.0, "kitty"),
        ];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.words_today, 24);
        assert_eq!(summary.total_words, 31);
    }

    #[test]
    fn month_boundary_does_not_break_the_series() {
        let entries = vec![
            test_entry("2026-08-31T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-01T10:00:00Z", 20, 4.0, "kitty"),
        ];
        let summary = summary(&entries, now_at("2026-09-01T12:00:00Z"), &cfg(), 2);
        assert_eq!(summary.series.len(), 2);
        assert_eq!(summary.series[0].day, "2026-08-31");
        assert_eq!(summary.series[0].words, 10);
        assert_eq!(summary.series[1].day, "2026-09-01");
        assert_eq!(summary.series[1].words, 20);
    }

    #[test]
    fn series_includes_days_without_entries() {
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty")];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 3);
        assert_eq!(summary.series.len(), 3);
        assert_eq!(summary.series[0].entries, 0);
        assert_eq!(summary.series[0].avg_wpm, None);
        assert_eq!(summary.series[2].entries, 1);
    }

    #[test]
    fn record_ignores_short_and_thin_utterances() {
        let mut entries = vec![
            // Полторы секунды, шесть слов — 240 wpm, но реплика слишком короткая.
            test_entry("2026-09-05T10:00:00Z", 6, 1.5, "kitty"),
            // Три слова за пять секунд — слов слишком мало.
            test_entry("2026-09-05T10:01:00Z", 3, 5.0, "kitty"),
            // Настоящий рекорд.
            test_entry("2026-09-05T10:02:00Z", 30, 10.0, "kitty"),
        ];
        entries[2].ts = now_at("2026-09-05T10:02:00Z");
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.record_wpm, Some(180.0));
        assert_eq!(summary.record_at, Some(now_at("2026-09-05T10:02:00Z")));
    }

    #[test]
    fn record_is_absent_when_nothing_qualifies() {
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 2, 4.0, "kitty")];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.record_wpm, None);
        assert_eq!(summary.record_at, None);
    }

    #[test]
    fn streak_counts_consecutive_days_back_from_today() {
        let entries = vec![
            test_entry("2026-09-01T10:00:00Z", 5, 4.0, "kitty"),
            // 2 сентября пропущено — серия обрывается здесь.
            test_entry("2026-09-03T10:00:00Z", 5, 4.0, "kitty"),
            test_entry("2026-09-04T10:00:00Z", 5, 4.0, "kitty"),
            test_entry("2026-09-05T10:00:00Z", 5, 4.0, "kitty"),
        ];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.streak_days, 3);
    }

    #[test]
    fn streak_survives_a_day_that_has_only_just_begun() {
        let entries = vec![
            test_entry("2026-09-04T10:00:00Z", 5, 4.0, "kitty"),
            test_entry("2026-09-05T10:00:00Z", 5, 4.0, "kitty"),
        ];
        // Сегодня 6-е и записей ещё нет: серия считается от вчера и равна 2.
        let summary = summary(&entries, now_at("2026-09-06T09:00:00Z"), &cfg(), 7);
        assert_eq!(summary.streak_days, 2);
    }

    #[test]
    fn streak_is_zero_after_a_full_missed_day() {
        let entries = vec![test_entry("2026-09-04T10:00:00Z", 5, 4.0, "kitty")];
        let summary = summary(&entries, now_at("2026-09-06T09:00:00Z"), &cfg(), 7);
        assert_eq!(summary.streak_days, 0);
    }

    #[test]
    fn saved_minutes_compare_dictation_with_typing_baseline() {
        // 80 слов за 60 с: набор на 40 wpm занял бы 2 минуты, диктовка — одну.
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 80, 60.0, "kitty")];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert!((summary.saved_minutes - 1.0).abs() < 1e-4, "{summary:?}");
        assert!((summary.minutes_recorded - 1.0).abs() < 1e-4);
    }

    #[test]
    fn zero_typing_baseline_does_not_divide_by_zero() {
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 80, 60.0, "kitty")];
        let cfg = StatsConfig {
            typing_baseline_wpm: 0,
        };
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg, 7);
        assert_eq!(summary.saved_minutes, 0.0);
    }

    #[test]
    fn by_app_groups_and_orders_by_words() {
        let mut entries = vec![
            test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T10:01:00Z", 40, 20.0, "firefox"),
            test_entry("2026-09-05T10:02:00Z", 5, 2.0, "kitty"),
        ];
        entries[2].app = None;
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.by_app.len(), 3);
        assert_eq!(summary.by_app[0].app, "firefox");
        assert_eq!(summary.by_app[0].words, 40);
        assert!(summary.by_app.iter().any(|a| a.app == "unknown"));
    }

    #[test]
    fn latency_and_tokens_are_summarised_over_the_window() {
        let mut first = test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty");
        first.latency_ms.stt = 400;
        first.latency_ms.total = 500;
        first.latency_ms.llm = Some(1000);
        first.latency_ms.inject = Some(100);
        first.tokens = Some(Tokens {
            prompt: 120,
            completion: 60,
        });
        let mut second = test_entry("2026-09-05T10:01:00Z", 10, 4.0, "kitty");
        second.latency_ms.stt = 600;
        second.latency_ms.total = 700;
        second.latency_ms.llm = None;
        second.latency_ms.inject = Some(200);

        let summary = summary(&[first, second], now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.latency_ms.stt, 500);
        assert_eq!(summary.latency_ms.total, 600);
        // Средняя по стадии считается только там, где стадия была.
        assert_eq!(summary.latency_ms.llm, Some(1000));
        assert_eq!(summary.latency_ms.inject, Some(150));
        assert_eq!(summary.tokens.prompt, 120);
        assert_eq!(summary.tokens.completion, 60);
    }

    #[test]
    fn empty_journal_gives_an_empty_but_valid_summary() {
        let summary = summary(&[], now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(summary.total_words, 0);
        assert_eq!(summary.avg_wpm_all, None);
        assert_eq!(summary.streak_days, 0);
        assert_eq!(summary.series.len(), 7);
        assert!(summary.by_app.is_empty());
        assert_eq!(summary.latency_ms.total, 0);
        // Ноль без минуса: «-0.0 мин» в отчёте выглядит как ошибка вычислений.
        assert!(summary.minutes_recorded.is_sign_positive(), "минус у нуля");
        assert!(summary.saved_minutes.is_sign_positive(), "минус у нуля");
        assert!(
            summary.series[0].audio_secs.is_sign_positive(),
            "минус у нуля"
        );
    }

    #[test]
    fn json_shape_matches_the_contract_for_the_gui() {
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty")];
        let summary = summary(&entries, now_at("2026-09-05T12:00:00Z"), &cfg(), 1);
        let value: serde_json::Value = serde_json::to_value(&summary).unwrap();
        let object = value.as_object().unwrap();
        let expected = [
            "total_words",
            "words_today",
            "avg_wpm_today",
            "avg_wpm_7d",
            "avg_wpm_all",
            "record_wpm",
            "record_at",
            "streak_days",
            "minutes_recorded",
            "saved_minutes",
            "latency_ms",
            "tokens",
            "series",
            "by_app",
        ];
        for key in expected {
            assert!(object.contains_key(key), "нет поля {key}: {value}");
        }
        assert_eq!(object.len(), expected.len(), "лишние поля: {value}");
        let day = &value["series"][0];
        for key in [
            "day",
            "entries",
            "words",
            "audio_secs",
            "avg_wpm",
            "avg_latency_ms",
        ] {
            assert!(day.get(key).is_some(), "нет поля series.{key}: {day}");
        }
        let app = &value["by_app"][0];
        for key in ["app", "entries", "words", "avg_wpm"] {
            assert!(app.get(key).is_some(), "нет поля by_app.{key}: {app}");
        }
        assert_eq!(value["latency_ms"]["stt"], 400);
        assert_eq!(value["tokens"]["prompt"], 0);
        assert_eq!(value["series"][0]["day"], "2026-09-05");
    }

    #[test]
    fn sessions_split_on_a_pause_longer_than_thirty_minutes() {
        let session = Uuid::new_v4();
        let mut entries = vec![
            test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T10:20:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T11:30:00Z", 10, 4.0, "kitty"),
        ];
        for entry in &mut entries {
            entry.session_id = session;
        }
        let sessions = sessions(&entries);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].entries, 2);
        assert_eq!(sessions[0].words, 20);
        assert_eq!(sessions[1].entries, 1);
        assert_eq!(sessions[0].ended, now_at("2026-09-05T10:20:00Z"));
    }

    #[test]
    fn sessions_split_on_a_new_daemon_run_even_without_a_pause() {
        let mut entries = vec![
            test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"),
            test_entry("2026-09-05T10:01:00Z", 10, 4.0, "kitty"),
        ];
        entries[0].session_id = Uuid::new_v4();
        entries[1].session_id = Uuid::new_v4();
        assert_eq!(sessions(&entries).len(), 2);
        assert!(sessions(&[]).is_empty());
    }

    #[test]
    fn reset_marker_hides_older_entries_without_deleting_them() {
        let directory = tempfile::tempdir().unwrap();
        let journal_path = directory.path().join("journal.jsonl");
        assert_eq!(read_reset_marker(&journal_path), None);

        let entries = vec![
            test_entry("2026-09-04T10:00:00Z", 100, 40.0, "kitty"),
            test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty"),
        ];
        write_reset_marker(&journal_path, now_at("2026-09-05T00:00:00Z")).unwrap();
        let marker = read_reset_marker(&journal_path);
        assert_eq!(marker, Some(now_at("2026-09-05T00:00:00Z")));
        assert!(reset_marker_path(&journal_path).exists());

        let after =
            summary_since_reset(&entries, marker, now_at("2026-09-05T12:00:00Z"), &cfg(), 7);
        assert_eq!(after.total_words, 10);
        // Сами записи на месте.
        assert_eq!(entries.len(), 2);

        clear_reset_marker(&journal_path).unwrap();
        assert_eq!(read_reset_marker(&journal_path), None);
        clear_reset_marker(&journal_path).unwrap();
    }

    #[test]
    fn corrupt_reset_marker_is_treated_as_no_reset() {
        let directory = tempfile::tempdir().unwrap();
        let journal_path = directory.path().join("journal.jsonl");
        std::fs::write(reset_marker_path(&journal_path), "не json").unwrap();
        assert_eq!(read_reset_marker(&journal_path), None);
    }

    #[test]
    fn csv_export_has_the_documented_columns() {
        let entries = vec![test_entry("2026-09-05T10:00:00Z", 10, 4.0, "kitty")];
        let csv = export_csv(&entries);
        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "ts,wpm,words,audio_secs,app,style,latency_total_ms"
        );
        let row = lines.next().unwrap();
        assert!(row.contains("150.0"), "{row}");
        assert!(row.contains("kitty"), "{row}");
        assert!(lines.next().is_none());
    }

    #[test]
    fn sparkline_scales_from_zero_to_the_maximum() {
        assert_eq!(sparkline(&[]), "");
        assert_eq!(sparkline(&[0.0, 0.0]), "▁▁");
        assert_eq!(sparkline(&[0.0, 10.0]), "▁█");
        let line = sparkline(&[0.0, 5.0, 10.0]);
        assert_eq!(line.chars().count(), 3);
        assert_eq!(line.chars().nth(2), Some('█'));
    }
}
