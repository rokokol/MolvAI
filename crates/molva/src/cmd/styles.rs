// SPDX-License-Identifier: MIT
//! `molva styles` — какие профили постобработки есть и что они говорят модели.

use clap::Subcommand;
use molva_core::app::styles::Styles;
use molva_core::Config;

#[derive(Debug, Subcommand)]
pub enum StylesAction {
    /// Показать все стили
    List,
    /// Показать системный промпт одного стиля
    Show { id: String },
}

pub fn run(action: StylesAction, config: &Config) -> anyhow::Result<()> {
    let styles = Styles::from_config(&config.style);
    match action {
        StylesAction::List => {
            println!("{:<12}  {:<14}  МОДЕЛЬ", "ID", "НАЗВАНИЕ");
            for style in styles.all() {
                let current = if style.id == config.style.default {
                    "по умолчанию"
                } else {
                    ""
                };
                println!(
                    "{:<12}  {:<14}  {:<7}  {}",
                    style.id,
                    style.name,
                    if style.uses_llm { "да" } else { "нет" },
                    current
                );
            }
            if !config.style.by_app.is_empty() {
                println!("\nПо приложениям:");
                for (app, style) in &config.style.by_app {
                    println!("  {app} → {style}");
                }
            }
            Ok(())
        }
        StylesAction::Show { id } => {
            let style = styles
                .get(&id)
                .ok_or_else(|| anyhow::anyhow!("стиля {id} нет: посмотрите `molva styles list`"))?;
            println!("id:      {}", style.id);
            println!("имя:     {}", style.name);
            println!("модель:  {}", if style.uses_llm { "да" } else { "нет" });
            if style.uses_llm {
                println!("промпт:\n{}", style.system_prompt);
            }
            Ok(())
        }
    }
}
