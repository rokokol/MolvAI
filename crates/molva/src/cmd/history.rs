// SPDX-License-Identifier: MIT
//! `molva history` — просмотр, поиск, фильтрация и выгрузка истории реплик.

use std::path::PathBuf;

use chrono::Utc;
use clap::{Args, Subcommand, ValueEnum};
use molva_core::app::journal::{self, FileJournal};
use molva_core::domain::entry::Entry;
use molva_core::domain::inject::{OutputMode, TextInjector};
use molva_core::domain::notify::Notifier;
use molva_core::infra::inject::ClipboardOnlyInjector;
use molva_core::infra::ipc::{Client, IpcClientError};
use molva_core::infra::notify::LogNotifier;
use molva_core::ipc::protocol::Command;
use molva_core::Config;

use super::{
    confirm, one_line, open_journal, parse_moment, truncate, CmdError, ID_SEPARATOR,
    LINE_TEXT_LIMIT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// Таблица для человека.
    Table,
    /// Массив записей журнала как есть.
    Json,
    /// Одна строка на реплику.
    Plain,
    /// То же, что plain: формат рассчитан на rofi и dmenu.
    Rofi,
}

#[derive(Debug, Args)]
pub(crate) struct HistoryArgs {
    /// Сколько последних записей показать
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Искать подстроку в тексте реплики
    #[arg(long)]
    pub search: Option<String>,
    /// Только реплики из этого приложения
    #[arg(long)]
    pub app: Option<String>,
    /// С какого момента: 7d, 24h, 2026-09-01
    #[arg(long)]
    pub since: Option<String>,
    /// По какой момент: 2026-09-05
    #[arg(long)]
    pub until: Option<String>,
    #[arg(long, value_enum, default_value_t = Format::Table)]
    pub format: Format,
    #[command(subcommand)]
    pub action: Option<HistoryAction>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum HistoryAction {
    /// Показать одну запись целиком
    Show {
        /// Идентификатор или его начало
        id: String,
    },
    /// Вставить реплику из истории заново, не диктуя её ещё раз
    Paste {
        /// Идентификатор или его начало
        id: Option<String>,
        /// Взять последнюю реплику
        #[arg(long)]
        last: bool,
    },
    /// Удалить одну запись
    Delete { id: String },
    /// Очистить историю целиком
    Clear {
        #[arg(long)]
        yes: bool,
    },
    /// Выгрузить историю в .jsonl или .csv
    Export { file: PathBuf },
}

pub(crate) fn run(
    args: HistoryArgs,
    config: &Config,
    socket: &std::path::Path,
) -> anyhow::Result<()> {
    let journal = open_journal(config)?;
    match args.action {
        None => list(&journal, &args, config),
        Some(HistoryAction::Show { id }) => show(&journal, &id),
        Some(HistoryAction::Paste { id, last }) => paste(&journal, id.as_deref(), last, socket),
        Some(HistoryAction::Delete { id }) => delete(&journal, &id),
        Some(HistoryAction::Clear { yes }) => clear(&journal, yes),
        Some(HistoryAction::Export { file }) => export(&journal, &file),
    }
}

/// Отобрать записи по фильтрам и вернуть последние `limit` в хронологическом порядке.
pub(crate) fn select(entries: &[Entry], args: &HistoryArgs) -> anyhow::Result<Vec<Entry>> {
    let now = Utc::now();
    let mut selected = entries.to_vec();
    if let Some(needle) = &args.search {
        selected = journal::search_entries(&selected, needle);
    }
    if let Some(app) = &args.app {
        selected = journal::filter_by_app(&selected, app);
    }
    let since = args
        .since
        .as_deref()
        .map(|value| parse_moment(value, now, false))
        .transpose()?;
    let until = args
        .until
        .as_deref()
        .map(|value| parse_moment(value, now, true))
        .transpose()?;
    if since.is_some() || until.is_some() {
        selected = journal::filter_by_date(&selected, since, until);
    }
    if args.limit > 0 && selected.len() > args.limit {
        selected = selected.split_off(selected.len() - args.limit);
    }
    Ok(selected)
}

fn list(journal: &FileJournal, args: &HistoryArgs, config: &Config) -> anyhow::Result<()> {
    let entries = select(&journal.load_all()?, args)?;
    if entries.is_empty() {
        if matches!(args.format, Format::Table) {
            println!("История пуста. Журнал: {}", journal.path().display());
            if !config.journal.enabled {
                println!("Журнал выключен настройкой journal.enabled = false.");
            }
        }
        return Ok(());
    }
    print!("{}", render(&entries, args.format)?);
    Ok(())
}

/// Отрисовать записи в выбранном формате.
pub(crate) fn render(entries: &[Entry], format: Format) -> anyhow::Result<String> {
    Ok(match format {
        Format::Json => format!("{}\n", serde_json::to_string_pretty(entries)?),
        Format::Plain | Format::Rofi => {
            entries.iter().map(line).collect::<Vec<String>>().join("\n") + "\n"
        }
        Format::Table => table(entries),
    })
}

/// Строка формата `plain`/`rofi`: время, скорость, текст и идентификатор в хвосте.
pub(crate) fn line(entry: &Entry) -> String {
    let text = one_line(entry.text_final.as_deref().unwrap_or(""));
    format!(
        "{}  {}  {}  {ID_SEPARATOR}{}",
        entry.ts.format("%Y-%m-%d %H:%M"),
        wpm_cell(entry),
        truncate(&text, LINE_TEXT_LIMIT),
        entry.id
    )
}

fn wpm_cell(entry: &Entry) -> String {
    match entry.wpm {
        Some(wpm) => format!("{wpm:.0} wpm"),
        None => "— wpm".to_string(),
    }
}

fn table(entries: &[Entry]) -> String {
    let mut out = format!(
        "{:<16}  {:>9}  {:>5}  {:<12}  {:<10}  {}\n",
        "ВРЕМЯ (UTC)", "СКОРОСТЬ", "СЛОВ", "ПРИЛОЖЕНИЕ", "СТИЛЬ", "ТЕКСТ"
    );
    for entry in entries {
        let text = one_line(entry.text_final.as_deref().unwrap_or("(текст не сохранён)"));
        out.push_str(&format!(
            "{:<16}  {:>9}  {:>5}  {:<12}  {:<10}  {}\n",
            entry.ts.format("%Y-%m-%d %H:%M"),
            wpm_cell(entry),
            entry.words,
            truncate(entry.app.as_deref().unwrap_or("—"), 12),
            truncate(&entry.style, 10),
            truncate(&text, 60)
        ));
    }
    out
}

/// Найти запись по идентификатору или его началу.
pub(crate) fn find<'a>(entries: &'a [Entry], id: &str) -> anyhow::Result<&'a Entry> {
    let id = id.trim_start_matches(ID_SEPARATOR);
    let matches: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.id.to_string().starts_with(id))
        .collect();
    match matches.len() {
        0 => anyhow::bail!("записи {id} в истории нет"),
        1 => Ok(matches[0]),
        many => anyhow::bail!("под {id} подходит {many} записей, уточните идентификатор"),
    }
}

fn show(journal: &FileJournal, id: &str) -> anyhow::Result<()> {
    let entries = journal.load_all()?;
    let entry = find(&entries, id)?;
    println!("{}", serde_json::to_string_pretty(entry)?);
    Ok(())
}

/// Выбрать реплику для повтора: по идентификатору или последнюю.
///
/// Чистая функция: правило выбора проверяется без журнала и без демона.
pub(crate) fn pick_for_paste<'a>(
    entries: &'a [Entry],
    id: Option<&str>,
    last: bool,
) -> anyhow::Result<&'a Entry> {
    if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
        return find(entries, id);
    }
    if !last {
        anyhow::bail!("укажите идентификатор реплики или --last");
    }
    entries
        .last()
        .ok_or_else(|| anyhow::anyhow!("история пуста: повторять нечего"))
}

/// Текст реплики для повтора; запись без текста повторить нельзя, и это не «пустая вставка».
pub(crate) fn text_to_paste(entry: &Entry) -> anyhow::Result<&str> {
    let text = entry
        .text_final
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    text.ok_or_else(|| {
        anyhow::anyhow!(
            "в записи {} нет текста: журнал вёлся с journal.include_text = false",
            entry.id
        )
    })
}

/// Что сказать, когда демона нет: текст уже в буфере обмена, повторять диктовку не нужно.
fn daemon_is_down(err: &IpcClientError) -> CmdError {
    CmdError::no_daemon(format!(
        "демон не запущен ({err}); текст реплики скопирован в буфер обмена, нажмите Ctrl+V"
    ))
}

/// Повторить реплику из истории: вставка идёт тем же способом, что и у обычной диктовки.
///
/// Критерий AM-17: повтор доступен из истории, а когда демон недоступен — текст остаётся
/// в буфере обмена, и команда честно сообщает об этом кодом выхода 3, а не молчаливым успехом.
fn paste(
    journal: &FileJournal,
    id: Option<&str>,
    last: bool,
    socket: &std::path::Path,
) -> anyhow::Result<()> {
    let entries = journal.load_all()?;
    let entry = pick_for_paste(&entries, id, last)?;
    let text = text_to_paste(entry)?.to_string();

    let mut client = match Client::connect(socket) {
        Ok(client) => client,
        Err(err) => {
            copy_to_clipboard(&text)?;
            return Err(daemon_is_down(&err).into());
        }
    };
    match client.call_ok(Command::InjectText {
        text: text.clone(),
        mode: None,
    }) {
        Ok(result) => {
            let method = result
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("вставка");
            println!("Реплика {} вставлена заново ({method}).", entry.id);
            Ok(())
        }
        Err(err) => {
            copy_to_clipboard(&text)?;
            Err(CmdError::no_daemon(format!(
                "демон не вставил текст ({err}); он скопирован в буфер обмена, нажмите Ctrl+V"
            ))
            .into())
        }
    }
}

/// Положить текст в буфер обмена: последний рубеж, когда вставить некуда.
fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    let notifier: std::sync::Arc<dyn Notifier> = std::sync::Arc::new(LogNotifier);
    ClipboardOnlyInjector::system(notifier)
        .inject(text, OutputMode::Clipboard)
        .map_err(|err| CmdError::file(format!("буфер обмена недоступен: {err}")))?;
    Ok(())
}

fn delete(journal: &FileJournal, id: &str) -> anyhow::Result<()> {
    let entries = journal.load_all()?;
    let entry = find(&entries, id)?;
    let entry_id = entry.id;
    if journal.delete(entry_id)? {
        println!("Запись {entry_id} удалена.");
    } else {
        anyhow::bail!("запись {entry_id} не удалось удалить");
    }
    Ok(())
}

fn clear(journal: &FileJournal, yes: bool) -> anyhow::Result<()> {
    let count = journal.load_all()?.len();
    if count == 0 {
        println!("История уже пуста.");
        return Ok(());
    }
    if !confirm(&format!("Удалить все записи ({count})?"), yes)? {
        println!("Отменено.");
        return Ok(());
    }
    journal.clear()?;
    println!("История очищена: удалено записей — {count}.");
    Ok(())
}

fn export(journal: &FileJournal, file: &std::path::Path) -> anyhow::Result<()> {
    let is_csv = file
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"));
    let count = if is_csv {
        journal.export_csv(file)?
    } else {
        journal.export_jsonl(file)?
    };
    println!("Выгружено записей: {count} → {}", file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use molva_core::domain::entry::{LatencyMs, Mode, Source};
    use uuid::Uuid;

    fn entry(ts: &str, app: &str, text: &str) -> Entry {
        Entry {
            schema: 1,
            id: Uuid::new_v4(),
            ts: chrono::DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            session_id: Uuid::nil(),
            mode: Mode::Dictation,
            source: Source::Mic,
            app: Some(app.into()),
            language: Some("ru".into()),
            audio_secs: 4.0,
            words: 10,
            wpm: Entry::wpm_for(10, 4.0),
            style: "cleanup".into(),
            stt_engine: "fake".into(),
            stt_model: "fake".into(),
            llm_provider: None,
            llm_model: None,
            llm_used: false,
            local_llm: true,
            dict_hits: 0,
            inject_method: None,
            latency_ms: LatencyMs::default(),
            tokens: None,
            error: None,
            text_raw: None,
            text_final: Some(text.into()),
            audio_path: None,
        }
    }

    fn args() -> HistoryArgs {
        HistoryArgs {
            limit: 20,
            search: None,
            app: None,
            since: None,
            until: None,
            format: Format::Table,
            action: None,
        }
    }

    #[test]
    fn the_limit_keeps_the_newest_entries_in_order() {
        let entries: Vec<Entry> = (0..5)
            .map(|i| {
                entry(
                    &format!("2026-09-05T10:0{i}:00Z"),
                    "kitty",
                    &format!("текст {i}"),
                )
            })
            .collect();
        let mut args = args();
        args.limit = 2;
        let selected = select(&entries, &args).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].text_final.as_deref(), Some("текст 3"));
        assert_eq!(selected[1].text_final.as_deref(), Some("текст 4"));
    }

    #[test]
    fn search_and_app_filters_compose() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", "kitty", "собрание переносится"),
            entry("2026-09-05T10:01:00Z", "firefox", "собрание состоится"),
            entry("2026-09-05T10:02:00Z", "kitty", "отчёт готов"),
        ];
        let mut args = args();
        args.search = Some("собрание".into());
        args.app = Some("kitty".into());
        let selected = select(&entries, &args).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].text_final.as_deref(),
            Some("собрание переносится")
        );
    }

    #[test]
    fn the_date_range_is_inclusive_on_both_ends() {
        let entries = vec![
            entry("2026-09-01T10:00:00Z", "kitty", "первое"),
            entry("2026-09-05T10:00:00Z", "kitty", "пятое"),
            entry("2026-09-09T10:00:00Z", "kitty", "девятое"),
        ];
        let mut args = args();
        args.since = Some("2026-09-05".into());
        args.until = Some("2026-09-05".into());
        let selected = select(&entries, &args).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text_final.as_deref(), Some("пятое"));
    }

    #[test]
    fn a_bad_date_is_an_error_not_an_empty_list() {
        let mut args = args();
        args.since = Some("позавчера".into());
        assert!(select(&[], &args).is_err());
    }

    #[test]
    fn the_plain_line_carries_time_speed_text_and_id() {
        let entry = entry("2026-09-05T15:42:00Z", "kitty", "привет\nмир");
        let line = line(&entry);
        assert!(
            line.starts_with("2026-09-05 15:42  150 wpm  привет мир  "),
            "{line}"
        );
        assert!(
            line.ends_with(&format!("{ID_SEPARATOR}{}", entry.id)),
            "{line}"
        );
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_long_line_is_cut_but_the_id_survives() {
        let long = "слово ".repeat(60);
        let entry = entry("2026-09-05T15:42:00Z", "kitty", &long);
        let line = line(&entry);
        assert!(line.contains('…'), "{line}");
        assert!(line.ends_with(&entry.id.to_string()), "{line}");
    }

    #[test]
    fn an_entry_without_speed_still_renders() {
        let mut entry = entry("2026-09-05T15:42:00Z", "kitty", "ага");
        entry.wpm = None;
        assert!(line(&entry).contains("— wpm"));
    }

    #[test]
    fn the_json_format_is_a_valid_array_of_entries() {
        let entries = vec![entry("2026-09-05T10:00:00Z", "kitty", "текст")];
        let json = render(&entries, Format::Json).unwrap();
        let back: Vec<Entry> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn the_table_has_a_header_and_a_row_per_entry() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", "kitty", "первый"),
            entry("2026-09-05T10:01:00Z", "firefox", "второй"),
        ];
        let table = render(&entries, Format::Table).unwrap();
        assert_eq!(table.lines().count(), 3);
        assert!(table.starts_with("ВРЕМЯ (UTC)"), "{table}");
        assert!(table.contains("kitty"), "{table}");
    }

    #[test]
    fn paste_takes_the_last_entry_or_the_one_asked_for() {
        let entries = vec![
            entry("2026-09-05T10:00:00Z", "kitty", "первая"),
            entry("2026-09-05T10:01:00Z", "kitty", "последняя"),
        ];
        assert_eq!(
            pick_for_paste(&entries, None, true).unwrap().id,
            entries[1].id
        );
        let id = entries[0].id.to_string();
        assert_eq!(
            pick_for_paste(&entries, Some(&id[..8]), false).unwrap().id,
            entries[0].id
        );
    }

    #[test]
    fn paste_without_an_id_and_without_last_is_an_explicit_error() {
        let entries = vec![entry("2026-09-05T10:00:00Z", "kitty", "текст")];
        let err = pick_for_paste(&entries, None, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--last"), "{err}");
        let err = pick_for_paste(&[], None, true).unwrap_err().to_string();
        assert!(err.contains("пуста"), "{err}");
    }

    #[test]
    fn an_entry_without_text_cannot_be_repeated() {
        let mut without_text = entry("2026-09-05T10:00:00Z", "kitty", "текст");
        without_text.text_final = None;
        let err = text_to_paste(&without_text).unwrap_err().to_string();
        assert!(err.contains("include_text"), "{err}");
        let with_text = entry("2026-09-05T10:00:00Z", "kitty", "  реплика  ");
        assert_eq!(text_to_paste(&with_text).unwrap(), "реплика");
    }

    #[test]
    fn a_missing_daemon_is_reported_honestly_with_exit_code_3() {
        let dir = tempfile::tempdir().unwrap();
        let err = daemon_is_down(&IpcClientError::NotRunning {
            path: dir.path().join("absent.sock"),
            message: "сокета нет".into(),
        });
        assert_eq!(err.code, 3, "код выхода «демон недоступен»");
        assert!(err.message.contains("буфер обмена"), "{}", err.message);
    }

    #[test]
    fn an_entry_is_found_by_an_id_prefix() {
        let entries = vec![entry("2026-09-05T10:00:00Z", "kitty", "текст")];
        let id = entries[0].id.to_string();
        assert_eq!(find(&entries, &id[..8]).unwrap().id, entries[0].id);
        // Разделитель из формата rofi можно не отрезать вручную.
        assert_eq!(
            find(&entries, &format!("{ID_SEPARATOR}{id}")).unwrap().id,
            entries[0].id
        );
        assert!(find(&entries, "deadbeef").is_err());
    }
}
