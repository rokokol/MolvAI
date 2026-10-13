// SPDX-License-Identifier: MIT
//! `molva styles` — which post-processing styles exist and what they tell the model

use clap::Subcommand;
use molva_core::app::styles::Styles;
use molva_core::Config;

#[derive(Debug, Subcommand)]
pub(crate) enum StylesAction {
    /// Show all styles
    List,
    /// Show the system prompt of one style
    Show { id: String },
}

pub(crate) fn run(action: StylesAction, config: &Config) -> anyhow::Result<()> {
    let styles = Styles::from_config(&config.style);
    match action {
        StylesAction::List => {
            println!("{:<12}  {:<14}  MODEL", "ID", "NAME");
            for style in styles.all() {
                let current = if style.id == config.style.default {
                    "default"
                } else {
                    ""
                };
                println!(
                    "{:<12}  {:<14}  {:<7}  {}",
                    style.id,
                    style.name,
                    if style.uses_llm { "yes" } else { "no" },
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
    }
}
