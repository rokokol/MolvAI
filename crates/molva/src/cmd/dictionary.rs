// SPDX-License-Identifier: MIT
//! `molva dictionary` — terms that the recognizer hears wrong

use clap::Subcommand;
use molva_core::app::dictionary::{Dictionary, Term};
use molva_core::Config;

use super::{open_journal, truncate};

#[derive(Debug, Subcommand)]
pub(crate) enum DictionaryAction {
    /// Show the terms and their aliases
    List,
    /// Add a term
    Add {
        /// How the term must look in the text
        term: String,
        /// How the recognizer hears it; the flag can be repeated
        #[arg(long = "alias")]
        aliases: Vec<String>,
    },
    /// Reread the dictionary file and show the result
    Reload,
    /// The size of the dictionary and how many replacements it made
    Stats,
}

/// The dictionary lies next to the settings file that the command was started with
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
                println!("The dictionary is empty. File: {}", path.display());
                return Ok(());
            }
            println!("{:<24}  {:<7}  ALIASES", "TERM", "CASE");
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
                "Added: {term} ({} aliases) → {}",
                aliases.len(),
                path.display()
            );
            println!("The daemon picks up the dictionary by itself: it rereads the file by its modification time.");
            Ok(())
        }
        DictionaryAction::Reload => {
            let dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            println!(
                "Dictionary reread: terms — {}, file {}",
                dictionary.len(),
                path.display()
            );
            Ok(())
        }
        DictionaryAction::Stats => {
            let dictionary = Dictionary::load(&path, config.dictionary.fuzzy)?;
            let aliases: usize = dictionary.terms().iter().map(|t| t.aliases.len()).sum();
            println!("File:              {}", path.display());
            println!("Terms:             {}", dictionary.len());
            println!("Aliases:           {aliases}");
            println!(
                "Fuzzy search:      {}",
                if config.dictionary.fuzzy { "on" } else { "off" }
            );
            println!(
                "In the STT prompt: {}",
                if config.dictionary.in_prompt {
                    "yes"
                } else {
                    "no"
                }
            );

            match open_journal(config).and_then(|journal| Ok(journal.load_all()?)) {
                Ok(entries) => {
                    let hits: u64 = entries.iter().map(|e| u64::from(e.dict_hits)).sum();
                    let touched = entries.iter().filter(|e| e.dict_hits > 0).count();
                    println!(
                        "Replacements:      {hits} in {touched} of {} phrases",
                        entries.len()
                    );
                }
                Err(error) => println!("History unavailable: {error}"),
            }
            Ok(())
        }
    }
}
