// SPDX-License-Identifier: MIT
//! `molva styles` — which post-processing styles exist and what they tell the model

use clap::Subcommand;
use molva_core::app::styles::Styles;
use molva_core::infra::ipc::Client;
use molva_core::ipc::protocol::Command;
use molva_core::Config;

#[derive(Debug, Subcommand)]
pub(crate) enum StylesAction {
    /// Show all styles
    List,
    /// Show the system prompt of one style
    Show { id: String },
    /// Switch to the next style in a circle
    Next,
}

pub(crate) fn run(
    action: StylesAction,
    config: &Config,
    socket: &std::path::Path,
) -> anyhow::Result<()> {
    let styles = Styles::from_config(&config.style);
    match action {
        StylesAction::List => {
            println!("{:<12}  {:<14}  MODEL   MIN WORDS", "ID", "NAME");
            for style in styles.all() {
                let current = if style.id == config.style.default {
                    "default"
                } else {
                    ""
                };
                println!(
                    "{:<12}  {:<14}  {:<7} {:<9} {}",
                    style.id,
                    style.name,
                    if style.uses_llm { "yes" } else { "no" },
                    if style.uses_llm {
                        style.llm_min_words.to_string()
                    } else {
                        "-".into()
                    },
                    current
                );
            }
            if !config.style.by_app.is_empty() {
                println!("\nBy application:");
                for (app, style) in &config.style.by_app {
                    println!("  {app} → {style}");
                }
            }
            Ok(())
        }
        StylesAction::Show { id } => {
            let style = styles.get(&id).ok_or_else(|| {
                anyhow::anyhow!("there is no style {id}: see `molva styles list`")
            })?;
            println!("id:      {}", style.id);
            println!("name:    {}", style.name);
            println!("model:   {}", if style.uses_llm { "yes" } else { "no" });
            println!("min words: {}", style.llm_min_words);
            if style.uses_llm {
                for (lang, prompt) in &style.system_prompts {
                    println!("prompt ({lang}):\n{prompt}");
                }
                if style.system_prompts.is_empty() {
                    println!("prompt:\n{}", style.system_prompt);
                } else {
                    println!("prompt (other languages):\n{}", style.system_prompt);
                }
            }
            Ok(())
        }
        StylesAction::Next => {
            let mut client = Client::connect(socket)?;
            let answer = client.call_ok(Command::StyleNext)?;
            if let Some(style) = answer.get("style").and_then(|value| value.as_str()) {
                println!("{style}");
            }
            Ok(())
        }
    }
}
