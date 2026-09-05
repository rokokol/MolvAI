// SPDX-License-Identifier: MIT
//! Сводка статистики для вкладки Stats.
//!
//! Форма `StatsSummary` — контракт с дорожкой D. Когда появится
//! `molva_core::app::stats::summary(&[Entry], DateTime<Utc>, &StatsConfig, u32)`,
//! удаляется ровно одна функция — [`summary`], а типы переезжают в ядро.
//! Дни считаются по UTC, как и `now` в подписи ядра.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Days, NaiveDate, Utc};
use molva_core::config::StatsConfig;
use molva_core::domain::entry::MIN_AUDIO_SECS_FOR_WPM;
use molva_core::domain::Entry;
use serde::{Deserialize, Serialize};

/// Приложение, которое не удалось определить, показывается одной строкой.
pub const UNKNOWN_APP: &str = "unknown";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatencySummary {
    pub stt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inject: Option<u32>,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TokenSummary {
    pub prompt: u64,
    pub completion: u64,
}

/// Точка графика: один день.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayPoint {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub entries: u32,
    pub words: u64,
    pub audio_secs: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_wpm: Option<f32>,
    pub avg_latency_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppRow {
    pub app: String,
    pub entries: u32,
    pub words: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_wpm: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSummary {
    pub total_words: u64,
    pub words_today: u64,
    pub avg_wpm_today: Option<f32>,
    pub avg_wpm_7d: Option<f32>,
    pub avg_wpm_all: Option<f32>,
    pub record_wpm: Option<f32>,
    /// RFC 3339.
    pub record_at: Option<DateTime<Utc>>,
    pub streak_days: u32,
    pub minutes_recorded: f32,
    pub saved_minutes: f32,
    pub latency_ms: LatencySummary,
    pub tokens: TokenSummary,
    pub series: Vec<DayPoint>,
    pub by_app: Vec<AppRow>,
}

/// Средний темп по набору реплик: суммарные слова на суммарное аудио.
///
/// Реплики короче секунды не участвуют: доля секунды в знаменателе даёт бессмысленные числа.
fn avg_wpm<'a>(entries: impl Iterator<Item = &'a Entry>) -> Option<f32> {
    let mut words = 0u64;
    let mut secs = 0f32;
    for entry in entries.filter(|e| e.audio_secs >= MIN_AUDIO_SECS_FOR_WPM) {
        words += u64::from(entry.words);
        secs += entry.audio_secs;
    }
    if secs <= 0.0 {
        return None;
    }
    Some(words as f32 / secs * 60.0)
}

fn day_of(entry: &Entry) -> NaiveDate {
    entry.ts.date_naive()
}

fn mean_u32(values: &[u32]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let sum: u64 = values.iter().map(|v| u64::from(*v)).sum();
    (sum / values.len() as u64) as u32
}

/// Длина серии подряд идущих дней с репликами, считая от сегодня (или от вчера,
/// если сегодня ещё не диктовали — начатый день серию не обрывает).
fn streak(days: &BTreeMap<NaiveDate, Vec<&Entry>>, today: NaiveDate) -> u32 {
    let mut cursor = if days.contains_key(&today) {
        today
    } else {
        match today.checked_sub_days(Days::new(1)) {
            Some(day) if days.contains_key(&day) => day,
            _ => return 0,
        }
    };
    let mut count = 0u32;
    while days.contains_key(&cursor) {
        count += 1;
        match cursor.checked_sub_days(Days::new(1)) {
            Some(prev) => cursor = prev,
            None => break,
        }
    }
    count
}

/// Сводка по журналу. `range_days` задаёт длину ряда для графика (0 — только сегодня).
///
/// Заменяется на `molva_core::app::stats::summary` после слияния дорожки D.
pub fn summary(
    entries: &[Entry],
    now: DateTime<Utc>,
    config: &StatsConfig,
    range_days: u32,
) -> StatsSummary {
    let today = now.date_naive();
    let mut by_day: BTreeMap<NaiveDate, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        by_day.entry(day_of(entry)).or_default().push(entry);
    }

    let total_words: u64 = entries.iter().map(|e| u64::from(e.words)).sum();
    let today_entries: Vec<&Entry> = by_day.get(&today).cloned().unwrap_or_default();
    let words_today: u64 = today_entries.iter().map(|e| u64::from(e.words)).sum();

    let week_start = today.checked_sub_days(Days::new(6)).unwrap_or(today);
    let last_7d: Vec<&Entry> = entries
        .iter()
        .filter(|e| day_of(e) >= week_start && day_of(e) <= today)
        .collect();

    let record = entries
        .iter()
        .filter(|e| e.audio_secs >= MIN_AUDIO_SECS_FOR_WPM)
        .filter_map(|e| e.wpm.map(|wpm| (wpm, e.ts)))
        .fold(None::<(f32, DateTime<Utc>)>, |best, current| match best {
            Some(best) if best.0 >= current.0 => Some(best),
            _ => Some(current),
        });

    let audio_secs: f32 = entries.iter().map(|e| e.audio_secs).sum();
    let minutes_recorded = audio_secs / 60.0;
    let baseline = config.typing_baseline_wpm.max(1);
    let typing_minutes = total_words as f32 / baseline as f32;
    let saved_minutes = (typing_minutes - minutes_recorded).max(0.0);

    let latency = LatencySummary {
        stt: mean_u32(&entries.iter().map(|e| e.latency_ms.stt).collect::<Vec<_>>()),
        llm: {
            let values: Vec<u32> = entries.iter().filter_map(|e| e.latency_ms.llm).collect();
            (!values.is_empty()).then(|| mean_u32(&values))
        },
        inject: {
            let values: Vec<u32> = entries.iter().filter_map(|e| e.latency_ms.inject).collect();
            (!values.is_empty()).then(|| mean_u32(&values))
        },
        total: mean_u32(
            &entries
                .iter()
                .map(|e| e.latency_ms.total)
                .collect::<Vec<_>>(),
        ),
    };

    let tokens = entries.iter().filter_map(|e| e.tokens.as_ref()).fold(
        TokenSummary::default(),
        |mut acc, tokens| {
            acc.prompt += u64::from(tokens.prompt);
            acc.completion += u64::from(tokens.completion);
            acc
        },
    );

    // Ряд строится по календарю, а не по наличию записей: пустые дни — нули на графике.
    let span = range_days.max(1);
    let first_day = today
        .checked_sub_days(Days::new(u64::from(span - 1)))
        .unwrap_or(today);
    let mut series = Vec::with_capacity(span as usize);
    let mut day = first_day;
    while day <= today {
        let of_day = by_day.get(&day).cloned().unwrap_or_default();
        series.push(DayPoint {
            day: format!("{:04}-{:02}-{:02}", day.year(), day.month(), day.day()),
            entries: of_day.len() as u32,
            words: of_day.iter().map(|e| u64::from(e.words)).sum(),
            audio_secs: of_day.iter().map(|e| e.audio_secs).sum(),
            avg_wpm: avg_wpm(of_day.iter().copied()),
            avg_latency_ms: mean_u32(
                &of_day
                    .iter()
                    .map(|e| e.latency_ms.total)
                    .collect::<Vec<_>>(),
            ),
        });
        match day.checked_add_days(Days::new(1)) {
            Some(next) => day = next,
            None => break,
        }
    }

    let mut apps: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        let key = entry.app.clone().unwrap_or_else(|| UNKNOWN_APP.to_string());
        apps.entry(key).or_default().push(entry);
    }
    let mut by_app: Vec<AppRow> = apps
        .into_iter()
        .map(|(app, rows)| AppRow {
            app,
            entries: rows.len() as u32,
            words: rows.iter().map(|e| u64::from(e.words)).sum(),
            avg_wpm: avg_wpm(rows.iter().copied()),
        })
        .collect();
    by_app.sort_by(|a, b| b.words.cmp(&a.words).then_with(|| a.app.cmp(&b.app)));

    StatsSummary {
        total_words,
        words_today,
        avg_wpm_today: avg_wpm(today_entries.iter().copied()),
        avg_wpm_7d: avg_wpm(last_7d.iter().copied()),
        avg_wpm_all: avg_wpm(entries.iter()),
        record_wpm: record.map(|(wpm, _)| wpm),
        record_at: record.map(|(_, ts)| ts),
        streak_days: streak(&by_day, today),
        minutes_recorded,
        saved_minutes,
        latency_ms: latency,
        tokens,
        series,
        by_app,
    }
}

/// CSV дневного ряда: заголовок и по строке на день, разделитель — запятая.
pub fn series_to_csv(series: &[DayPoint]) -> String {
    let mut out = String::from("day,entries,words,audio_secs,avg_wpm,avg_latency_ms\n");
    for point in series {
        let wpm = point.avg_wpm.map(|v| format!("{v:.1}")).unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{:.1},{},{}\n",
            point.day, point.entries, point.words, point.audio_secs, wpm, point.avg_latency_ms
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::domain::entry::{LatencyMs, Mode, Source, Tokens, SCHEMA_VERSION};
    use uuid::Uuid;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn entry(ts: &str, words: u32, audio_secs: f32, app: Option<&str>) -> Entry {
        Entry {
            schema: SCHEMA_VERSION,
            id: Uuid::new_v4(),
            ts: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            app: app.map(str::to_string),
            language: Some("ru".into()),
            audio_secs,
            words,
            wpm: Entry::wpm_for(words, audio_secs),
            style: "cleanup".into(),
            stt_engine: "whisper-cpp".into(),
            stt_model: "small".into(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs {
                stt: 400,
                rules: 1,
                total: 500,
                ..Default::default()
            },
            tokens: None,
            error: None,
            text_raw: None,
            text_final: None,
            audio_path: None,
        }
    }

    #[test]
    fn empty_journal_gives_zeroes_not_panics() {
        let s = summary(&[], now(), &StatsConfig::default(), 7);
        assert_eq!(s.total_words, 0);
        assert_eq!(s.avg_wpm_all, None);
        assert_eq!(s.streak_days, 0);
        assert_eq!(s.series.len(), 7);
        assert!(s.by_app.is_empty());
    }

    #[test]
    fn average_wpm_is_total_words_over_total_audio() {
        // 60 слов за 60 секунд ⇒ ровно 60 WPM, независимо от разбиения по репликам.
        let entries = [
            entry("2026-09-05T10:00:00Z", 20, 20.0, None),
            entry("2026-09-05T11:00:00Z", 40, 40.0, None),
        ];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.avg_wpm_all, Some(60.0));
        assert_eq!(s.words_today, 60);
    }

    #[test]
    fn replies_shorter_than_a_second_do_not_skew_the_average() {
        let entries = [
            entry("2026-09-05T10:00:00Z", 10, 10.0, None),
            entry("2026-09-05T10:01:00Z", 3, 0.2, None),
        ];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.avg_wpm_all, Some(60.0));
        // Слова короткой реплики из общего счёта не выкидываются.
        assert_eq!(s.total_words, 13);
    }

    #[test]
    fn series_covers_the_whole_range_including_empty_days() {
        let entries = [entry("2026-09-05T10:00:00Z", 10, 10.0, None)];
        let s = summary(&entries, now(), &StatsConfig::default(), 30);
        assert_eq!(s.series.len(), 30);
        assert_eq!(s.series[0].day, "2026-08-07");
        assert_eq!(s.series[29].day, "2026-09-05");
        assert_eq!(s.series[29].words, 10);
        assert_eq!(s.series[0].words, 0);
        assert_eq!(s.series[0].avg_wpm, None);
    }

    #[test]
    fn streak_counts_consecutive_days_back_from_today() {
        let entries = [
            entry("2026-09-05T10:00:00Z", 5, 5.0, None),
            entry("2026-09-04T10:00:00Z", 5, 5.0, None),
            entry("2026-09-03T10:00:00Z", 5, 5.0, None),
            entry("2026-09-01T10:00:00Z", 5, 5.0, None),
        ];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.streak_days, 3);
    }

    #[test]
    fn streak_survives_a_day_that_has_not_started_yet() {
        let entries = [entry("2026-09-04T10:00:00Z", 5, 5.0, None)];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.streak_days, 1);
    }

    #[test]
    fn streak_is_zero_after_a_gap() {
        let entries = [entry("2026-09-02T10:00:00Z", 5, 5.0, None)];
        assert_eq!(
            summary(&entries, now(), &StatsConfig::default(), 7).streak_days,
            0
        );
    }

    #[test]
    fn record_is_the_fastest_reply_with_its_date() {
        let entries = [
            entry("2026-09-01T10:00:00Z", 100, 30.0, None),
            entry("2026-09-02T10:00:00Z", 50, 30.0, None),
        ];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.record_wpm, Some(200.0));
        assert_eq!(
            s.record_at.unwrap().to_rfc3339(),
            "2026-09-01T10:00:00+00:00"
        );
    }

    #[test]
    fn saved_time_compares_speech_with_the_typing_baseline() {
        // 400 слов при базе 40 WPM — это 10 минут набора против 1 минуты речи.
        let entries = [entry("2026-09-05T10:00:00Z", 400, 60.0, None)];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert!((s.minutes_recorded - 1.0).abs() < 1e-4);
        assert!((s.saved_minutes - 9.0).abs() < 1e-4, "{}", s.saved_minutes);
    }

    #[test]
    fn saved_time_never_goes_negative() {
        let entries = [entry("2026-09-05T10:00:00Z", 1, 600.0, None)];
        assert_eq!(
            summary(&entries, now(), &StatsConfig::default(), 7).saved_minutes,
            0.0
        );
    }

    #[test]
    fn by_app_is_sorted_by_words_and_names_the_unknown_app() {
        let entries = [
            entry("2026-09-05T10:00:00Z", 10, 10.0, Some("kitty")),
            entry("2026-09-05T10:01:00Z", 50, 10.0, Some("firefox")),
            entry("2026-09-05T10:02:00Z", 5, 10.0, None),
        ];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.by_app[0].app, "firefox");
        assert_eq!(s.by_app[0].words, 50);
        assert_eq!(s.by_app[2].app, UNKNOWN_APP);
    }

    #[test]
    fn absent_llm_latency_stays_absent_instead_of_zero() {
        let entries = [entry("2026-09-05T10:00:00Z", 10, 10.0, None)];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.latency_ms.llm, None);
        assert_eq!(s.latency_ms.stt, 400);
    }

    #[test]
    fn tokens_are_summed_over_entries_that_have_them() {
        let mut with_tokens = entry("2026-09-05T10:00:00Z", 10, 10.0, None);
        with_tokens.tokens = Some(Tokens {
            prompt: 100,
            completion: 20,
        });
        let entries = [with_tokens, entry("2026-09-05T10:01:00Z", 10, 10.0, None)];
        let s = summary(&entries, now(), &StatsConfig::default(), 7);
        assert_eq!(s.tokens.prompt, 100);
        assert_eq!(s.tokens.completion, 20);
    }

    #[test]
    fn serde_shape_matches_the_contract_with_track_d() {
        let s = summary(&[], now(), &StatsConfig::default(), 7);
        let json = serde_json::to_value(&s).unwrap();
        for key in [
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
        ] {
            assert!(json.get(key).is_some(), "нет поля {key}");
        }
        assert!(json["latency_ms"].get("stt").is_some());
        assert!(json["tokens"].get("prompt").is_some());
        assert_eq!(json["avg_wpm_today"], serde_json::Value::Null);
    }

    #[test]
    fn csv_has_a_header_and_one_line_per_day() {
        let s = summary(
            &[entry("2026-09-05T10:00:00Z", 10, 10.0, None)],
            now(),
            &StatsConfig::default(),
            7,
        );
        let csv = series_to_csv(&s.series);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 8);
        assert!(lines[0].starts_with("day,entries,words"));
        assert!(lines[7].starts_with("2026-09-05,1,10"));
    }
}
