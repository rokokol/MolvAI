// SPDX-License-Identifier: MIT
//! `molva dictionary` — термины, которые распознаватель слышит неверно.

use clap::Subcommand;
use molva_core::app::dictionary::{Dictionary, Term};
use molva_core::Config;

use super::{open_journal, truncate};

#[derive(Debug, Subcommand)]
pub(crate) enum DictionaryAction {
    /// Показать термины и их алиасы
    List,
    /// Добавить термин
    Add {
        /// Как термин должен выглядеть в тексте
        term: String,
        /// Как его слышит распознаватель; флаг можно повторять
        #[arg(long = "alias")]
        aliases: Vec<String>,
    },
    /// Перечитать файл словаря и показать, что получилось
    Reload,
    /// Размер словаря и сколько подстановок он сделал
    Stats,
}

/// Словарь лежит рядом с тем файлом настроек, с которым запущена команда.
pub(crate) fn run(
    action: DictionaryAction,
    config: &Config,
    config_path: &std::path::Path,
) -> anyhow::Result<()> {
    let path = config.dictionary_path_near(config_path)?;
    match action {
        DictionaryAction::List => {
            let dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            if dictionary.is_empty() {
                println!("Словарь пуст. Файл: {}", path.display());
                return Ok(());
            }
            println!("{:<24}  {:<7}  АЛИАСЫ", "ТЕРМИН", "РЕГИСТР");
            for term in dictionary.terms() {
                println!(
                    "{:<24}  {:<7}  {}",
                    truncate(&term.word, 24),
                    format!("{:?}", term.case).to_lowercase(),
                    term.aliases.join(", ")
                );
            }
            Ok(())
        }
        DictionaryAction::Add { term, aliases } => {
            let mut dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            let refs: Vec<&str> = aliases.iter().map(String::as_str).collect();
            dictionary.add(Term::new(&term, &refs))?;
            println!(
                "Добавлено: {term} ({} алиасов) → {}",
                aliases.len(),
                path.display()
            );
            println!("Демон подхватит словарь сам: он перечитывает файл по времени изменения.");
            Ok(())
        }
        DictionaryAction::Reload => {
            let dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            println!(
                "Словарь перечитан: терминов — {}, файл {}",
                dictionary.len(),
                path.display()
            );
            Ok(())
        }
        DictionaryAction::Stats => {
            let dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            let aliases: usize = dictionary.terms().iter().map(|t| t.aliases.len()).sum();
            println!("Файл:              {}", path.display());
            println!("Терминов:          {}", dictionary.len());
            println!("Алиасов:           {aliases}");
            println!(
                "Нечёткий поиск:    {}",
                if config.dictionary.fuzzy {
                    "включён"
                } else {
                    "выключен"
                }
            );
            println!(
                "В подсказке STT:   {}",
                if config.dictionary.in_prompt {
                    "да"
                } else {
                    "нет"
                }
            );

            match open_journal(config).and_then(|journal| Ok(journal.load_all()?)) {
                Ok(entries) => {
                    let hits: u64 = entries.iter().map(|e| u64::from(e.dict_hits)).sum();
                    let touched = entries.iter().filter(|e| e.dict_hits > 0).count();
                    println!(
                        "Подстановок всего: {hits} в {touched} репликах из {}",
                        entries.len()
                    );
                }
                Err(err) => println!("История недоступна: {err}"),
            }
            Ok(())
        }
    }
}
