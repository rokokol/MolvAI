// SPDX-License-Identifier: MIT
//! `molva daemon` — фоновый процесс, который держит микрофон, модель и сокет.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use molva_core::app::daemon::{Daemon, DaemonParts};
use molva_core::app::dictionary::Dictionary;
use molva_core::app::engine;
use molva_core::app::journal::{FileJournal, NullJournal};
use molva_core::app::pipeline::{Pipeline, PipelineConfig};
use molva_core::app::secrets;
use molva_core::config::Config;
use molva_core::domain::audio::AudioSource;
use molva_core::domain::clock::SystemClock;
use molva_core::domain::fakes::FakeAudioSource;
use molva_core::domain::journal::Journal;
use molva_core::domain::llm::LlmClient;
use molva_core::domain::notify::Notifier;
use molva_core::domain::stt::SttEngine;
use molva_core::infra::audio::CpalSource;
use molva_core::infra::inject::ChainInjector;
use molva_core::infra::ipc::{self, Server};
use molva_core::infra::llm::openai_compat::OpenAiCompatClient;
use molva_core::infra::notify::{LogNotifier, SystemNotifier};
use molva_core::infra::platform;
use molva_core::infra::sound;
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

/// Движок распознавания по настройкам: единая фабрика ядра, та же, что у `transcribe` и `bench`.
pub fn build_stt(config: &Config) -> anyhow::Result<Box<dyn SttEngine>> {
    engine::build_stt(config, None).map_err(|err| anyhow!("{err}"))
}

/// Источник звука: микрофон через cpal, а для фейкового движка — две секунды тишины,
/// чтобы сквозной путь проверялся без оборудования.
fn build_audio(config: &Config) -> Box<dyn AudioSource> {
    if config.stt.engine == engine::FAKE_ENGINE {
        return Box::new(FakeAudioSource::silence(2.0));
    }
    Box::new(CpalSource::new(
        &config.audio.device,
        config.audio.gain,
        config.audio.max_duration_secs,
    ))
}

/// Журнал реплик: файл JSONL или «никуда» в режиме без записи.
fn build_journal(config: &Config) -> anyhow::Result<Box<dyn Journal>> {
    if config.privacy.no_record_mode || !config.journal.enabled {
        return Ok(Box::new(NullJournal));
    }
    let path = config.journal_path()?;
    let journal = FileJournal::open_with(&path, config.journal.include_text)
        .with_context(|| format!("журнал {}", path.display()))?;
    Ok(Box::new(journal))
}

/// Модель постобработки, если она включена; ключ берётся из окружения по имени из настроек.
fn build_llm(config: &Config) -> Option<Arc<dyn LlmClient>> {
    if !config.llm.enabled || !config.privacy.send_to_llm {
        return None;
    }
    match OpenAiCompatClient::from_config(&config.llm, secrets::api_key(&config.llm)) {
        Ok(client) => Some(Arc::new(client)),
        Err(err) => {
            tracing::warn!(error = %err, "модель постобработки недоступна, работаем без неё");
            None
        }
    }
}

/// Словарь терминов; отсутствующий файл — пустой словарь, а не ошибка.
fn build_dictionary(config: &Config, config_path: &Path) -> Dictionary {
    let path = match config.dictionary_path_near(config_path) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(error = %err, "путь к словарю не определён, словарь пустой");
            return Dictionary::empty();
        }
    };
    match Dictionary::load(&path, config.dictionary.fuzzy) {
        Ok(dictionary) => dictionary,
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "словарь не прочитан, словарь пустой");
            Dictionary::empty()
        }
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
        engine = %config.stt.engine,
        model = %config.stt.model,
        "демон запускается"
    );

    let injector = ChainInjector::for_platform(&config.output, &detected, notifier.clone());
    let mut pipeline = Pipeline::new(
        build_stt(&config)?,
        build_llm(&config),
        Box::new(injector),
        build_journal(&config)?,
        clock.clone(),
        PipelineConfig::from_config(&config),
    )
    .with_dictionary(build_dictionary(&config, config_path));
    pipeline.set_session_id(session_id);

    let daemon = Daemon::spawn(DaemonParts {
        audio: build_audio(&config),
        processor: Box::new(pipeline),
        notifier,
        // Звук начала и конца записи; `audio.sounds = false` даёт молчаливую реализацию.
        sound: sound::build_sound_cue(&config.audio),
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
    fn the_fake_engine_is_available_and_missing_weights_say_how_to_get_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.engine = "fake".into();
        match build_stt(&config) {
            Ok(engine) => assert_eq!(engine.id(), "fake"),
            Err(err) => panic!("движок fake обязан собираться: {err}"),
        }
        config.stt.engine = "whisper-cpp".into();
        config.stt.model_path = dir.path().display().to_string();
        let err = match build_stt(&config) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("без файла весов движок собираться не должен"),
        };
        assert!(err.contains("molva models pull"), "{err}");
    }

    #[test]
    fn journal_is_silent_in_no_record_mode_and_a_file_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.journal.path = dir.path().join("journal.jsonl").display().to_string();
        config.privacy.no_record_mode = true;
        build_journal(&config).unwrap();
        assert!(!dir.path().join("journal.jsonl").exists());

        config.privacy.no_record_mode = false;
        build_journal(&config).unwrap();
        assert!(dir.path().join("journal.jsonl").exists());
    }

    #[test]
    fn llm_is_absent_unless_enabled() {
        let config = Config::default();
        assert!(build_llm(&config).is_none());
    }

    #[test]
    fn missing_dictionary_file_gives_an_empty_dictionary() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let config = Config::default();
        let dictionary = build_dictionary(&config, &config_path);
        assert_eq!(dictionary.apply("привет").1, 0);
    }
}
