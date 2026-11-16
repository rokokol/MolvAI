// SPDX-License-Identifier: MIT
//! `molva doctor` — what works on this system, what does not, and why
//!
//! The diagnostics print not "ok/not ok" but the reason and the next step: a user on NixOS without
//! a udev rule must understand from the output what exactly to add

use std::path::Path;
use std::time::Duration;

use molva_core::app::engine::{self, EngineChoice};
use molva_core::app::models;
use molva_core::app::probe;
use molva_core::app::secrets::{self, OsKeyring, SecretStore};
use molva_core::domain::llm::LlmError;
use molva_core::infra::inject::clipboard::SystemClipboard;
use molva_core::infra::ipc;
use molva_core::infra::llm::openai_compat::{OpenAiCompatClient, Provider};
use molva_core::infra::platform::{self, Platform, Tools};
use molva_core::Config;

/// How long the diagnostics wait for the model provider: longer than this already means the
/// provider does not answer
const LLM_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// One line of the report
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl Check {
    fn new(name: &str, ok: bool, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok,
            detail: detail.into(),
        }
    }

    pub(crate) fn line(&self) -> String {
        let mark = if self.ok { "yes" } else { "no" };
        format!("{:<24} {:<4} {}", self.name, mark, self.detail)
    }
}

/// Whether `/dev/uinput` is writable: the node can exist and still refuse the open
pub(crate) fn uinput_check() -> Check {
    let path = Path::new("/dev/uinput");
    if !path.exists() {
        return Check::new("/dev/uinput", false, "no uinput module in the kernel");
    }
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => Check::new("/dev/uinput", true, "opens for writing"),
        Err(error) => Check::new(
            "/dev/uinput",
            false,
            format!("{error}; a udev rule for the input group is needed"),
        ),
    }
}

/// Keyboards in `/dev/input` that the user is allowed to read
pub(crate) fn input_devices_check() -> Check {
    #[cfg(target_os = "linux")]
    {
        use molva_core::infra::hotkeys::evdev_source::EvdevHotkeys;
        let devices = EvdevHotkeys::devices();
        if devices.is_empty() {
            return Check::new(
                "/dev/input",
                false,
                "keyboards are not readable: add the user to the input group",
            );
        }
        Check::new("/dev/input", true, format!("keyboards: {}", devices.len()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Check::new("/dev/input", false, "Linux only")
    }
}

/// The whisper weights: the configured model is found on the disk and its SHA-256 matches the
/// catalog
pub(crate) fn whisper_model_check(config: &Config) -> Check {
    let name = config.stt.model.trim();
    let info = match models::find(name) {
        Ok(info) => info,
        Err(error) => return Check::new("whisper model", false, error.to_string()),
    };
    let path = match models::installed_path(config, name) {
        Ok(path) => path,
        Err(error) => return Check::new("whisper model", false, error.to_string()),
    };
    match models::verify(&path, &info.sha256) {
        Ok(true) => Check::new(
            "whisper model",
            true,
            format!("{name}: {}, sha256 matches", path.display()),
        ),
        Ok(false) => Check::new(
            "whisper model",
            false,
            format!(
                "{}: sha256 does not match the catalog; download again: molva models pull {name} --force",
                path.display()
            ),
        ),
        Err(error) => Check::new("whisper model", false, error.to_string()),
    }
}

/// The program that the default GigaAM model runs
///
/// The weights can be in place and the program absent, and then the daemon starts and looks
/// healthy while every phrase fails. The line names the way out, because the error of the first
/// phrase sends the person here
pub(crate) fn transcribe_cli_check(config: &Config) -> Check {
    const NAME: &str = "transcribe-cli";
    const NEXT: &str =
        "install transcribe.cpp, set stt.transcribe_cli, or choose a whisper model in stt.model";
    let configured = config.stt.transcribe_cli.trim();
    match probe::transcribe_cli_program(configured) {
        Some(path) => Check::new(NAME, true, path.display().to_string()),
        None if configured.is_empty() => Check::new(
            NAME,
            false,
            format!("not found beside the molva binary or in PATH; {NEXT}"),
        ),
        None => Check::new(NAME, false, format!("{configured} is not there; {NEXT}")),
    }
}

/// The language model: switched off in the settings, or the provider answers and knows the
/// configured model
pub(crate) fn llm_check(config: &Config) -> Check {
    if !config.llm.enabled {
        return Check::new(
            "LLM",
            true,
            "switched off in the settings (llm.enabled = false), the dictionary and the rules edit the text",
        );
    }
    let provider = Provider::parse(&config.llm.provider);
    let base_url =
        molva_core::infra::llm::openai_compat::configured_base_url(&config.llm).to_string();
    let client = match OpenAiCompatClient::new(
        base_url.clone(),
        config.llm.model.clone(),
        secrets::api_key(&config.llm),
        LLM_PROBE_TIMEOUT,
        provider.id(),
        molva_core::infra::llm::openai_compat::is_loopback_url(&base_url),
    ) {
        Ok(client) => client.with_cloud_consent(config.privacy.cloud_consent),
        Err(error) => return client_not_built(&error),
    };
    match client.list_models() {
        Ok(names) if names.iter().any(|name| name == &config.llm.model) => Check::new(
            "LLM",
            true,
            format!(
                "{} answers at {base_url}, the model {} is in place",
                provider.id(),
                config.llm.model
            ),
        ),
        Ok(names) => Check::new(
            "LLM",
            false,
            format!(
                "{} answers, but the model {} is missing (available: {}); for Ollama: ollama pull {}",
                provider.id(),
                config.llm.model,
                if names.is_empty() {
                    "nothing".to_string()
                } else {
                    names.join(", ")
                },
                config.llm.model
            ),
        ),
        Err(LlmError::Auth) => Check::new(
            "LLM",
            false,
            format!(
                "{base_url}: the key was not accepted; check llm.api_key_source and the variable {}",
                config.llm.api_key_env
            ),
        ),
        Err(error) => Check::new(
            "LLM",
            false,
            format!("{error}; start the provider or switch off llm.enabled"),
        ),
    }
}

/// The HTTP client was not built: the request did not reach the provider, so the advice is
/// about the environment of this machine and not about starting the model
pub(crate) fn client_not_built(error: &LlmError) -> Check {
    Check::new(
        "LLM",
        false,
        format!(
            "{error}; the HTTP client was not created: check the system certificates (SSL_CERT_FILE) \
             and the proxy variables, or switch off llm.enabled"
        ),
    )
}

/// The OS key store: write, read and delete of a probe entry. An unavailable store is
/// a "no" with an explanation, not a crash: without it the keys are read from environment
/// variables
pub(crate) fn keyring_check(store: &dyn SecretStore) -> Check {
    const NAME: &str = "key store";
    const HINT: &str = "start gnome-keyring/kwallet or use api_key_source = env";
    let probe = "molva-doctor-probe";
    let value = format!("probe-{}", std::process::id());
    let round_trip = store
        .set(probe, &value)
        .and_then(|()| store.get(probe))
        .and_then(|read| store.delete(probe).map(|()| read));
    match round_trip {
        Ok(Some(read)) if read == value => Check::new(NAME, true, "write, read and delete"),
        Ok(_) => Check::new(
            NAME,
            false,
            format!("the probe entry was read back wrong; {HINT}"),
        ),
        Err(error) => Check::new(NAME, false, format!("{error}; {HINT}")),
    }
}

/// The full report: lines in the order in which they are read
pub(crate) fn checks(socket: &Path, config: &Config) -> Vec<Check> {
    checks_with(socket, config, &OsKeyring)
}

/// The same with an explicit store: tests do not touch the OS store
pub(crate) fn checks_with(socket: &Path, config: &Config, store: &dyn SecretStore) -> Vec<Check> {
    let platform = platform::detect();
    let tools = Tools::detect();
    let mut checks = vec![
        Check::new("session", platform != Platform::Headless, platform.label()),
        Check::new(
            "window",
            true,
            platform::active_window_class().unwrap_or_else(|| "class unknown".into()),
        ),
        Check::new("hyprctl", tools.hyprctl, tool_detail(tools.hyprctl)),
        Check::new("wtype", tools.wtype, tool_detail(tools.wtype)),
        Check::new("ydotool", tools.ydotool, tool_detail(tools.ydotool)),
        Check::new("wl-copy", tools.wl_copy, tool_detail(tools.wl_copy)),
        Check::new(
            "clipboard",
            SystemClipboard::available(),
            "arboard or wl-copy",
        ),
        uinput_check(),
        input_devices_check(),
    ];
    checks.push(match ipc::ping(socket) {
        Some(pid) => Check::new("daemon", true, format!("pid {pid}, {}", socket.display())),
        None => Check::new(
            "daemon",
            false,
            format!(
                "does not answer on {}; start molva daemon",
                socket.display()
            ),
        ),
    });
    checks.push(whisper_model_check(config));
    if engine::resolve_engine(config, &EngineChoice::default()) == "transcribe-cpp" {
        checks.push(transcribe_cli_check(config));
    }
    checks.push(llm_check(config));
    checks.push(keyring_check(store));
    checks
}

fn tool_detail(found: bool) -> &'static str {
    if found {
        "found in PATH"
    } else {
        "not found in PATH"
    }
}

// A signature shared by all subcommands: the dispatcher in `main` calls them all the same way
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn run(socket: &Path, config: &Config) -> anyhow::Result<()> {
    for check in checks(socket, config) {
        println!("{}", check.line());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use molva_core::app::secrets::MemoryStore;

    use super::*;

    #[test]
    fn a_report_line_shows_the_name_the_verdict_and_the_reason() {
        let check = Check::new("wtype", false, "not found in PATH");
        let line = check.line();
        assert!(line.starts_with("wtype"), "{line}");
        assert!(line.contains(" no "), "{line}");
        assert!(line.contains("not found"), "{line}");
    }

    /// Settings with the models in a temporary directory and the LLM switched off:
    /// the diagnostics must reach neither into the home directory nor into the network
    fn offline_config(directory: &Path) -> Config {
        let mut config = Config::default();
        config.stt.model_path = directory.display().to_string();
        config.llm.enabled = false;
        config
    }

    #[test]
    fn the_report_covers_session_tools_daemon_model_and_llm() {
        let directory = tempfile::tempdir().unwrap();
        let checks = checks_with(
            &directory.path().join("absent.sock"),
            &offline_config(directory.path()),
            &MemoryStore::new(),
        );
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        for expected in [
            "session",
            "hyprctl",
            "wtype",
            "/dev/uinput",
            "daemon",
            "whisper model",
            "transcribe-cli",
            "LLM",
            "key store",
        ] {
            assert!(names.contains(&expected), "{names:?}");
        }
    }

    #[test]
    fn a_whisper_setting_leaves_the_gigaam_program_out_of_the_report() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = offline_config(directory.path());
        config.stt.engine = "whisper-cpp".into();
        config.stt.model = "small".into();
        let checks = checks_with(
            &directory.path().join("absent.sock"),
            &config,
            &MemoryStore::new(),
        );
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        assert!(!names.contains(&"transcribe-cli"), "{names:?}");
    }

    #[test]
    fn a_missing_program_of_the_default_model_names_the_way_out() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = offline_config(directory.path());
        let missing = directory.path().join("no-such-cli");
        config.stt.transcribe_cli = missing.display().to_string();
        let check = transcribe_cli_check(&config);
        assert!(!check.ok);
        assert!(
            check.detail.contains(&missing.display().to_string()),
            "{}",
            check.detail
        );
        assert!(check.detail.contains("transcribe.cpp"), "{}", check.detail);
        assert!(check.detail.contains("stt.model"), "{}", check.detail);
    }

    #[test]
    fn the_verdict_of_the_program_follows_the_search() {
        // The default setting names no program, so the answer comes from this machine: the line
        // must wrap that answer as it is, and a missing one must carry the next step
        let directory = tempfile::tempdir().unwrap();
        let config = offline_config(directory.path());
        assert!(config.stt.transcribe_cli.is_empty());
        let check = transcribe_cli_check(&config);
        let found = probe::transcribe_cli_program(&config.stt.transcribe_cli);
        assert_eq!(check.ok, found.is_some(), "{}", check.detail);
        if let Some(path) = found {
            assert_eq!(check.detail, path.display().to_string());
        } else {
            assert!(check.detail.contains("transcribe.cpp"), "{}", check.detail);
            assert!(check.detail.contains("stt.model"), "{}", check.detail);
        }
    }

    #[test]
    fn a_found_program_is_reported_with_its_path() {
        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("transcribe-cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        let mut config = offline_config(directory.path());
        config.stt.transcribe_cli = cli.display().to_string();
        let check = transcribe_cli_check(&config);
        assert!(check.ok, "{}", check.detail);
        assert_eq!(check.detail, cli.display().to_string());
    }

    #[test]
    fn the_keyring_check_round_trips_and_leaves_no_probe_behind() {
        let store = MemoryStore::new();
        let check = keyring_check(&store);
        assert!(check.ok, "{}", check.detail);
        assert_eq!(store.get("molva-doctor-probe").unwrap(), None);
    }

    #[test]
    fn an_unavailable_keyring_names_the_next_step_instead_of_failing() {
        let check = keyring_check(&MemoryStore::failing("no Secret Service"));
        assert!(!check.ok);
        assert!(
            check.detail.contains("no Secret Service"),
            "{}",
            check.detail
        );
        assert!(
            check.detail.contains("api_key_source = env"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn a_missing_model_names_the_pull_command() {
        let directory = tempfile::tempdir().unwrap();
        let check = whisper_model_check(&offline_config(directory.path()));
        assert!(!check.ok);
        assert!(
            check.detail.contains("molva models pull"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn a_model_with_a_wrong_hash_is_not_accepted() {
        let directory = tempfile::tempdir().unwrap();
        let config = offline_config(directory.path());
        let path = models::model_path(&config, &config.stt.model).unwrap();
        std::fs::write(&path, b"not really whisper weights").unwrap();
        let check = whisper_model_check(&config);
        assert!(!check.ok);
        assert!(check.detail.contains("sha256"), "{}", check.detail);
        assert!(check.detail.contains("--force"), "{}", check.detail);
    }

    #[test]
    fn a_disabled_llm_is_fine_and_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let check = llm_check(&offline_config(directory.path()));
        assert!(check.ok);
        assert!(check.detail.contains("llm.enabled"), "{}", check.detail);
    }

    #[test]
    fn an_unreachable_provider_is_reported_with_the_next_step() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = offline_config(directory.path());
        config.llm.enabled = true;
        config.llm.api_key_source = "env".into();
        config.llm.base_url = "http://127.0.0.1:9/v1".into();
        let check = llm_check(&config);
        assert!(!check.ok);
        assert!(check.detail.contains("llm.enabled"), "{}", check.detail);
    }

    #[test]
    fn a_client_that_cannot_be_built_names_the_environment_and_the_way_out() {
        let error = LlmError::Unavailable("builder error: no CA certificates found".into());
        let check = client_not_built(&error);
        assert!(!check.ok);
        assert!(
            check.detail.contains("no CA certificates found"),
            "{}",
            check.detail
        );
        assert!(check.detail.contains("certificates"), "{}", check.detail);
        assert!(check.detail.contains("llm.enabled"), "{}", check.detail);
    }

    #[test]
    fn a_missing_daemon_is_reported_with_the_socket_path_and_what_to_do() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let checks = checks(&socket, &offline_config(directory.path()));
        let daemon = checks.iter().find(|c| c.name == "daemon").unwrap();
        assert!(!daemon.ok);
        assert!(daemon.detail.contains("molva daemon"), "{}", daemon.detail);
        assert!(
            daemon.detail.contains(&socket.display().to_string()),
            "{}",
            daemon.detail
        );
    }

    #[test]
    fn uinput_check_never_panics_and_always_explains_itself() {
        let check = uinput_check();
        assert!(!check.detail.is_empty());
    }
}
