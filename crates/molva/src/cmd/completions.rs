// SPDX-License-Identifier: MIT
//! `molva completions <shell>` — скрипт автодополнения для оболочки.
//!
//! Скрипт идёт в stdout, чтобы его можно было сразу перенаправить:
//! `molva completions zsh > ~/.zsh/completions/_molva`.

use std::io::Write;

use clap::CommandFactory;
use clap_complete::Shell;

use super::CmdError;

/// Сгенерировать дополнение для указанной оболочки.
pub(crate) fn run<C: CommandFactory>(
    shell: Shell,
    bin_name: &str,
    out: &mut dyn Write,
) -> Result<(), CmdError> {
    let mut command = C::command();
    clap_complete::generate(shell, &mut command, bin_name, out);
    out.flush().map_err(|e| CmdError::file(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(clap::Parser)]
    #[command(name = "molva-test")]
    struct Sample {
        #[arg(long)]
        language: Option<String>,
    }

    #[test]
    fn bash_completion_mentions_the_binary_and_its_flags() {
        let mut out = Vec::new();
        run::<Sample>(Shell::Bash, "molva", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("molva"), "{text}");
        assert!(text.contains("--language"), "{text}");
    }

    #[test]
    fn every_supported_shell_produces_a_script() {
        for shell in [
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let mut out = Vec::new();
            run::<Sample>(shell, "molva", &mut out).unwrap();
            assert!(!out.is_empty(), "пустой скрипт для {shell}");
        }
    }
}
