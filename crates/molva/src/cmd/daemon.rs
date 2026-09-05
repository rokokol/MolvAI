// SPDX-License-Identifier: MIT
//! `molva daemon` — фоновый процесс, который держит микрофон, модель и сокет.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use molva_core::app::daemon::{Daemon, DaemonParts, ProcessorConfig, SimpleProcessor};
use molva_core::config::Config;
use molva_core::domain::audio::AudioSource;
use molva_core::domain::clock::SystemClock;
use molva_core::domain::fakes::{FakeAudioSource, FakeStt, MemJournal};
use molva_core::domain::notify::Notifier;
use molva_core::domain::stt::SttEngine;
use molva_core::infra::inject::ChainInjector;
use molva_core::infra::ipc::{self, Server};
use molva_core::infra::notify::{LogNotifier, SystemNotifier};
use molva_core::infra::platform;
use uuid::Uuid;

/// Путь к сокету: флаг важнее переменной окружения, переменная — умолчания.
///
/// Ядро про окружение не знает специально: тест на сокете во временном каталоге не должен
/// зависеть от того, что кто-то экспортировал `MOLVA_SOCKET`.
pub fn resolve_socket(flag: Option<PathBuf>) -> PathBuf {
    if let Some(path) = flag {
        return path;
    }
    if let Some(path) = std::env::var_os("MOLVA_SOCKET") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    ipc::socket_path()
}

/// Движок распознавания по настройкам.
///
/// Пока собран только `fake`: настоящий whisper приносит дорожка A, и подключается он здесь же,
/// одной веткой — поэтому ошибка называет, что именно нужно поставить, а не «не поддерживается».
pub fn build_stt(config: &Config) -> anyhow::Result<Box<dyn SttEngine>> {
    match config.stt.engine.as_str() {
        "fake" => Ok(Box::new(FakeStt::returning("тестовая реплика"))),
        other => Err(anyhow!(
            "движок распознавания «{other}» в этой сборке недоступен; \
             поставьте stt.engine = \"fake\" в конфиге для проверки сквозного пути"
        )),
    }
}

/// Источник звука. Настоящий захват через cpal приносит дорожка A.
fn build_audio(config: &Config) -> anyhow::Result<Box<dyn AudioSource>> {
    match config.stt.engine.as_str() {
        "fake" => Ok(Box::new(FakeAudioSource::silence(2.0))),
        other => Err(anyhow!("для движка «{other}» нужен захват с микрофона")),
    }
}

pub struct Options {
    pub socket: PathBuf,
    pub foreground: bool,
}

pub fn run(config_path: &Path, options: Options) -> anyhow::Result<()> {
    let config = Config::load_or_create(config_path)
        .with_context(|| format!("настройки {}", config_path.display()))?;

    // Один экземпляр на пользователя: две копии подрались бы за микрофон и за вставку.
    if let Some(pid) = ipc::ping(&options.socket) {
        return Err(anyhow!(
            "демон уже запущен (pid {pid}), сокет {}",
            options.socket.display()
        ));
    }

    let notifier: Arc<dyn Notifier> = if options.foreground {
        Arc::new(LogNotifier)
    } else {
        Arc::new(SystemNotifier::new())
    };
    let clock = Arc::new(SystemClock);
    let session_id = Uuid::new_v4();
    let detected = platform::detect();
    tracing::info!(
        platform = %detected.label(),
        socket = %options.socket.display(),
        "демон запускается"
    );

    let injector = ChainInjector::for_platform(&config.output, &detected, notifier.clone());
    let processor = SimpleProcessor::new(
        build_stt(&config)?,
        injector,
        // Файловый журнал приносит дорожка C; до этого записи живут в памяти процесса.
        MemJournal::default(),
        clock.clone(),
        notifier.clone(),
        ProcessorConfig::from_config(&config, session_id),
    );

    let daemon = Daemon::spawn(DaemonParts {
        audio: build_audio(&config)?,
        processor: Box::new(processor),
        notifier,
        clock,
        config: config.clone(),
    });
    let handle = daemon.handle();

    let server = Server::bind(&options.socket)
        .with_context(|| format!("сокет {}", options.socket.display()))?;
    println!("MolvAI: демон слушает {}", options.socket.display());
    server.serve(Arc::new(handle))?;
    daemon.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_socket_flag_wins_over_everything() {
        let explicit = PathBuf::from("/tmp/molva-test.sock");
        assert_eq!(resolve_socket(Some(explicit.clone())), explicit);
    }

    #[test]
    fn without_a_flag_the_default_path_is_used() {
        // Значение переменной окружения тест не подменяет: результат либо она, либо умолчание.
        let path = resolve_socket(None);
        assert!(path.to_string_lossy().contains("molva"), "{path:?}");
    }

    #[test]
    fn the_fake_engine_is_available_and_the_rest_say_what_to_do() {
        let mut config = Config::default();
        config.stt.engine = "fake".into();
        match build_stt(&config) {
            Ok(engine) => assert_eq!(engine.id(), "fake"),
            Err(err) => panic!("движок fake обязан собираться: {err}"),
        }
        config.stt.engine = "whisper-cpp".into();
        let err = match build_stt(&config) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("движка whisper-cpp в этой сборке нет"),
        };
        assert!(err.contains("whisper-cpp"), "{err}");
        assert!(err.contains("stt.engine"), "{err}");
    }
}
