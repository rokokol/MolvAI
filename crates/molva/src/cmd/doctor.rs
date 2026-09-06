// SPDX-License-Identifier: MIT
//! `molva doctor` — что в этой системе работает, а что нет и почему.
//!
//! Диагностика печатает не «ок/не ок», а причину и следующий шаг: пользователь на NixOS без
//! правила udev должен из вывода понять, что именно ему добавить.

use std::path::Path;
use std::time::Duration;

use molva_core::app::models;
use molva_core::app::secrets::{self, OsKeyring, SecretStore};
use molva_core::domain::llm::LlmError;
use molva_core::infra::inject::clipboard::SystemClipboard;
use molva_core::infra::ipc;
use molva_core::infra::llm::openai_compat::{OpenAiCompatClient, Provider};
use molva_core::infra::platform::{self, Platform, Tools};
use molva_core::Config;

/// Сколько диагностика ждёт провайдера модели: дольше — это уже «не отвечает».
const LLM_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Одна строка отчёта.
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
        let mark = if self.ok { "да" } else { "нет" };
        format!("{:<24} {:<4} {}", self.name, mark, self.detail)
    }
}

/// Доступность `/dev/uinput` на запись: файл может существовать и быть закрытым.
pub(crate) fn uinput_check() -> Check {
    let path = Path::new("/dev/uinput");
    if !path.exists() {
        return Check::new("/dev/uinput", false, "нет модуля uinput в ядре");
    }
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => Check::new("/dev/uinput", true, "открывается на запись"),
        Err(error) => Check::new(
            "/dev/uinput",
            false,
            format!("{error}; нужно правило udev на группу input"),
        ),
    }
}

/// Клавиатуры в `/dev/input`, которые нам разрешено читать.
pub(crate) fn input_devices_check() -> Check {
    #[cfg(target_os = "linux")]
    {
        use molva_core::infra::hotkeys::evdev_source::EvdevHotkeys;
        let devices = EvdevHotkeys::devices();
        if devices.is_empty() {
            return Check::new(
                "/dev/input",
                false,
                "клавиатуры не читаются: добавьте пользователя в группу input",
            );
        }
        Check::new("/dev/input", true, format!("клавиатур: {}", devices.len()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Check::new("/dev/input", false, "только Linux")
    }
}

/// Веса whisper: настроенная модель найдена на диске и её SHA-256 совпадает с каталогом.
pub(crate) fn whisper_model_check(config: &Config) -> Check {
    let name = config.stt.model.trim();
    let info = match models::find(name) {
        Ok(info) => info,
        Err(error) => return Check::new("модель whisper", false, error.to_string()),
    };
    let path = match models::installed_path(config, name) {
        Ok(path) => path,
        Err(error) => return Check::new("модель whisper", false, error.to_string()),
    };
    match models::verify(&path, info.sha256) {
        Ok(true) => Check::new(
            "модель whisper",
            true,
            format!("{name}: {}, sha256 совпадает", path.display()),
        ),
        Ok(false) => Check::new(
            "модель whisper",
            false,
            format!(
                "{}: sha256 не совпадает с каталогом; скачайте заново: molva models pull {name} --force",
                path.display()
            ),
        ),
        Err(error) => Check::new("модель whisper", false, error.to_string()),
    }
}

/// Языковая модель: выключена в настройках, либо провайдер отвечает и знает настроенную модель.
pub(crate) fn llm_check(config: &Config) -> Check {
    if !config.llm.enabled {
        return Check::new(
            "LLM",
            true,
            "выключена в настройках (llm.enabled = false), текст правят словарь и правила",
        );
    }
    let provider = Provider::parse(&config.llm.provider);
    let base_url = if config.llm.base_url.trim().is_empty() {
        provider.default_base_url().to_string()
    } else {
        config.llm.base_url.clone()
    };
    let client = match OpenAiCompatClient::new(
        base_url.clone(),
        config.llm.model.clone(),
        secrets::api_key(&config.llm),
        LLM_PROBE_TIMEOUT,
        provider.id(),
        provider.is_local(),
    ) {
        Ok(client) => client,
        Err(error) => return Check::new("LLM", false, error.to_string()),
    };
    match client.list_models() {
        Ok(names) if names.iter().any(|name| name == &config.llm.model) => Check::new(
            "LLM",
            true,
            format!(
                "{} отвечает на {base_url}, модель {} на месте",
                provider.id(),
                config.llm.model
            ),
        ),
        Ok(names) => Check::new(
            "LLM",
            false,
            format!(
                "{} отвечает, но модели {} нет (есть: {}); для Ollama: ollama pull {}",
                provider.id(),
                config.llm.model,
                if names.is_empty() {
                    "ничего".to_string()
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
                "{base_url}: ключ не принят; проверьте llm.api_key_source и переменную {}",
                config.llm.api_key_env
            ),
        ),
        Err(error) => Check::new(
            "LLM",
            false,
            format!("{error}; запустите провайдера или выключите llm.enabled"),
        ),
    }
}

/// Полный отчёт: строки в том порядке, в котором их читают.
/// Хранилище ключей ОС: запись, чтение и удаление пробной записи. Недоступное хранилище —
/// это «нет» с объяснением, а не падение: без него ключи читаются из переменных окружения.
pub(crate) fn keyring_check(store: &dyn SecretStore) -> Check {
    const NAME: &str = "хранилище ключей";
    const HINT: &str = "запустите gnome-keyring/kwallet или используйте api_key_source = env";
    let probe = "molva-doctor-probe";
    let value = format!("probe-{}", std::process::id());
    let round_trip = store
        .set(probe, &value)
        .and_then(|()| store.get(probe))
        .and_then(|read| store.delete(probe).map(|()| read));
    match round_trip {
        Ok(Some(read)) if read == value => Check::new(NAME, true, "запись, чтение и удаление"),
        Ok(_) => Check::new(
            NAME,
            false,
            format!("пробная запись прочиталась не той; {HINT}"),
        ),
        Err(error) => Check::new(NAME, false, format!("{error}; {HINT}")),
    }
}

pub(crate) fn checks(socket: &Path, config: &Config) -> Vec<Check> {
    checks_with(socket, config, &OsKeyring)
}

/// То же с явным хранилищем: тесты не трогают хранилище ОС.
pub(crate) fn checks_with(socket: &Path, config: &Config, store: &dyn SecretStore) -> Vec<Check> {
    let platform = platform::detect();
    let tools = Tools::detect();
    let mut checks = vec![
        Check::new("сессия", platform != Platform::Headless, platform.label()),
        Check::new(
            "окно",
            true,
            platform::active_window_class().unwrap_or_else(|| "класс неизвестен".into()),
        ),
        Check::new("hyprctl", tools.hyprctl, tool_detail(tools.hyprctl)),
        Check::new("wtype", tools.wtype, tool_detail(tools.wtype)),
        Check::new("ydotool", tools.ydotool, tool_detail(tools.ydotool)),
        Check::new("wl-copy", tools.wl_copy, tool_detail(tools.wl_copy)),
        Check::new(
            "буфер обмена",
            SystemClipboard::available(),
            "arboard или wl-copy",
        ),
        uinput_check(),
        input_devices_check(),
    ];
    checks.push(match ipc::ping(socket) {
        Some(pid) => Check::new("демон", true, format!("pid {pid}, {}", socket.display())),
        None => Check::new(
            "демон",
            false,
            format!(
                "не отвечает на {}; запустите molva daemon",
                socket.display()
            ),
        ),
    });
    checks.push(whisper_model_check(config));
    checks.push(llm_check(config));
    checks.push(keyring_check(store));
    checks
}

fn tool_detail(found: bool) -> &'static str {
    if found {
        "найден в PATH"
    } else {
        "не найден в PATH"
    }
}

// Общая для всех подкоманд сигнатура: диспетчер в `main` вызывает их одинаково.
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
        let check = Check::new("wtype", false, "не найден в PATH");
        let line = check.line();
        assert!(line.starts_with("wtype"), "{line}");
        assert!(line.contains("нет"), "{line}");
        assert!(line.contains("не найден"), "{line}");
    }

    /// Настройки, у которых модели лежат во временном каталоге, а LLM выключена:
    /// диагностика не должна лезть ни в домашний каталог, ни в сеть.
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
            "сессия",
            "hyprctl",
            "wtype",
            "/dev/uinput",
            "демон",
            "модель whisper",
            "LLM",
            "хранилище ключей",
        ] {
            assert!(names.contains(&expected), "{names:?}");
        }
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
        let check = keyring_check(&MemoryStore::failing("нет Secret Service"));
        assert!(!check.ok);
        assert!(
            check.detail.contains("нет Secret Service"),
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
    fn a_missing_daemon_is_reported_with_the_socket_path_and_what_to_do() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let checks = checks(&socket, &offline_config(directory.path()));
        let daemon = checks.iter().find(|c| c.name == "демон").unwrap();
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
