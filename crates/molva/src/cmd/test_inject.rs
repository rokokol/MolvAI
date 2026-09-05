// SPDX-License-Identifier: MIT
//! `molva test-inject` — проверить вставку, не поднимая демон и не говоря ни слова.
//!
//! Пауза перед вставкой обязательна: команду запускают из терминала, а проверять надо окно, в
//! которое пользователь успеет переключиться.

use std::sync::Arc;
use std::time::Duration;

use molva_core::config::Config;
use molva_core::domain::inject::{OutputMode, TextInjector};
use molva_core::domain::notify::Notifier;
use molva_core::infra::inject::{parse_output_mode, ChainInjector};
use molva_core::infra::notify::LogNotifier;
use molva_core::infra::platform;

pub const DEFAULT_TEXT: &str = "MolvAI: проверка вставки";
pub const DEFAULT_DELAY: Duration = Duration::from_secs(3);

pub fn run(
    config: &Config,
    mode: Option<&str>,
    text: Option<&str>,
    delay: Duration,
) -> anyhow::Result<()> {
    let text = text.unwrap_or(DEFAULT_TEXT);
    let resolved = resolve_mode(config, mode, text);
    let platform = platform::detect();
    let notifier: Arc<dyn Notifier> = Arc::new(LogNotifier);
    let mut injector = ChainInjector::for_platform(&config.output, &platform, notifier);

    println!(
        "сессия: {}; через {} с текст уйдёт в активное окно — переключитесь туда",
        platform.label(),
        delay.as_secs()
    );
    std::thread::sleep(delay);

    let window = platform::active_window_class();
    injector.apply_window(window.as_deref());
    let report = injector.inject(text, resolved)?;
    println!(
        "окно: {}; способ: {}",
        window.as_deref().unwrap_or("неизвестно"),
        report.method
    );
    for attempt in &report.attempts {
        println!("  не сработало — {attempt}");
    }
    Ok(())
}

/// Режим вставки для проверки: аргумент важнее конфига, `auto` разрешается по длине текста.
pub fn resolve_mode(config: &Config, mode: Option<&str>, text: &str) -> OutputMode {
    let requested = mode
        .map(parse_output_mode)
        .unwrap_or_else(|| parse_output_mode(&config.output.mode));
    requested.resolve(text, config.output.auto_type_max_chars as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_mode_wins_over_the_config() {
        let mut config = Config::default();
        config.output.mode = "clipboard".into();
        assert_eq!(
            resolve_mode(&config, Some("paste"), "текст"),
            OutputMode::Paste
        );
        assert_eq!(resolve_mode(&config, None, "текст"), OutputMode::Clipboard);
    }

    #[test]
    fn auto_from_the_config_is_resolved_by_the_length_of_the_text() {
        let config = Config::default();
        assert_eq!(resolve_mode(&config, None, "коротко"), OutputMode::Type);
        let long = "а".repeat(config.output.auto_type_max_chars as usize + 1);
        assert_eq!(resolve_mode(&config, None, &long), OutputMode::Paste);
    }

    #[test]
    fn the_default_text_says_where_it_came_from() {
        assert!(DEFAULT_TEXT.contains("MolvAI"));
        assert_eq!(DEFAULT_DELAY, Duration::from_secs(3));
    }
}
