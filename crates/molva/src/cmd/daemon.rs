// SPDX-License-Identifier: MIT
//! `molva daemon` — the background process that holds the microphone, the model and the socket

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context};
use molva_core::app::daemon::{Daemon, DaemonParts};
use molva_core::app::dictionary::Dictionary;
use molva_core::app::engine;
use molva_core::app::journal::{FileJournal, NullJournal};
use molva_core::app::models;
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
use molva_core::infra::ipc::{self, RequestHandler, Server};
use molva_core::infra::llm::openai_compat::OpenAiCompatClient;
use molva_core::infra::notify::{LogNotifier, SystemNotifier};
use molva_core::infra::platform;
use molva_core::infra::sound;
use uuid::Uuid;

/// The socket path: the flag wins over the environment variable, and the variable wins over the default
///
/// The core does not know about the environment on purpose: a test on a socket in a temporary directory must not
/// depend on whether someone exported `MOLVA_SOCKET`
pub(crate) fn resolve_socket(flag: Option<PathBuf>) -> PathBuf {
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

/// The recognition engine by the settings: the single factory of the core, the same as in `transcribe` and `bench`
pub(crate) fn build_stt(config: &Config) -> anyhow::Result<Box<dyn SttEngine>> {
    ensure_model_present(config)?;
    engine::build_stt(config, None).map_err(|err| anyhow!("{err}"))
}

/// Download the weights if they are missing (`stt.auto_pull`): the daemon on a clean machine must start
/// with one command and not send the person to read about `molva models pull`. The progress goes to
/// stderr as a bar, like in `molva models pull`; with the setting off, the behavior is as before:
/// a clear error with the command for the download
fn ensure_model_present(config: &Config) -> anyhow::Result<()> {
    if config.stt.engine.trim() != "whisper-cpp" || !config.stt.auto_pull {
        return Ok(());
    }
    let name = config.stt.model.trim();
    match models::installed_path(config, name) {
        Ok(_) => return Ok(()),
        Err(models::ModelError::NotInstalled { .. }) => {}
        Err(err) => return Err(anyhow!("{err}")),
    }
    let directory = models::models_dir(config)?;
    tracing::info!(model = name, directory = %directory.display(), "no weights, downloading");
    let report = super::models::pull(
        name,
        &directory,
        &config.stt.models_mirror,
        false,
        !super::progress_enabled(false),
    )
    .map_err(|err| anyhow!("{err}"))?;
    tracing::info!(path = %report.path.display(), "weights in place, checksum matches");
    Ok(())
}

/// The audio source: the microphone through cpal, and for the fake engine two seconds of silence,
/// so that the end-to-end path is checked without hardware
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

/// The phrase journal: a JSONL file, or "nowhere" in the no-record mode
fn build_journal(config: &Config) -> anyhow::Result<Box<dyn Journal>> {
    if config.privacy.no_record_mode || !config.journal.enabled {
        return Ok(Box::new(NullJournal));
    }
    let path = config.journal_path()?;
    // The core decides the path, the privacy and the cipher: the CLI and the GUI open the journal with the same function
    let journal =
        FileJournal::open_for(config).with_context(|| format!("journal {}", path.display()))?;
    Ok(Box::new(journal))
}

/// The post-processing model, if it is enabled; the key is taken from the environment by the name from the settings
fn build_llm(config: &Config) -> Option<Arc<dyn LlmClient>> {
    if !config.llm.enabled || !config.privacy.send_to_llm {
        return None;
    }
    match OpenAiCompatClient::from_config(&config.llm, secrets::api_key(&config.llm)) {
        Ok(client) => Some(Arc::new(client)),
        Err(err) => {
            tracing::warn!(error = %err, "post-processing model unavailable, working without it");
            None
        }
    }
}

/// The term dictionary; a missing file gives an empty dictionary, not an error
fn build_dictionary(config: &Config, config_path: &Path) -> Dictionary {
    let path = match config.dictionary_path_near(config_path) {
        Ok(path) => path,
        Err(err) => {
            tracing::warn!(error = %err, "dictionary path undefined, dictionary is empty");
            return Dictionary::empty();
        }
    };
    match Dictionary::load(&path, config.dictionary.fuzzy) {
        Ok(dictionary) => dictionary,
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "dictionary not read, dictionary is empty");
            Dictionary::empty()
        }
    }
}

/// One instance per user: the second copy exits with a message
///
/// The `single_instance` feature (criterion O-14): if a live daemon answers on the socket, the second
/// start does not begin at all, because two copies would fight for the microphone and for the paste. The check goes
/// through a ping of the socket and not through a pid file: a file survives the crash of a process, and an answer on the socket
/// does not. The exit code is separate, `7` (`exit::ALREADY_RUNNING`), so that the autostart tells
/// "already running" from a real error
pub(crate) fn single_instance_guard(
    socket: &Path,
    ping: impl Fn(&Path) -> Option<u32>,
) -> Result<(), crate::cmd::CmdError> {
    let Some(pid) = ping(socket) else {
        return Ok(());
    };
    Err(crate::cmd::CmdError::already_running(format!(
        "the daemon is already running (pid {pid}), socket {}",
        socket.display()
    )))
}

pub(crate) struct Options {
    pub socket: PathBuf,
    pub foreground: bool,
}

pub(crate) fn run(config_path: &Path, options: &Options) -> anyhow::Result<()> {
    let config = Config::load_or_create(config_path)
        .with_context(|| format!("settings {}", config_path.display()))?;

    single_instance_guard(&options.socket, ipc::ping)?;

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
        "daemon starting"
    );

    let injector = ChainInjector::for_platform(&config.output, &detected, notifier.clone());
    // The second instance is for repeating phrases from the history by `inject.text`: it must not wait
    // until the pipeline finishes its phrase
    let repeat_injector = ChainInjector::for_platform(&config.output, &detected, notifier.clone());
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
        // The sound of the start and the end of the recording; `audio.sounds = false` gives a silent implementation
        sound: sound::build_sound_cue(&config.audio),
        injector: Some(Box::new(repeat_injector)),
        clock,
        config: config.clone(),
    });
    let handle = daemon.handle();
    // `config.reload` and `style.set` reread the same file the daemon was started with
    handle.set_config_path(config_path.to_path_buf());

    let server = Server::bind(&options.socket)
        .with_context(|| format!("socket {}", options.socket.display()))?;
    println!("MolvAI: the daemon listens on {}", options.socket.display());
    let handler: Arc<dyn RequestHandler> = Arc::new(handle);
    server.serve(&handler)?;
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
        // The test does not override the environment variable: the result is either it or the default
        let path = resolve_socket(None);
        assert!(path.to_string_lossy().contains("molva"), "{path:?}");
    }

    #[test]
    fn a_second_instance_refuses_to_start_and_says_which_one_is_running() {
        // Criterion O-14: the second copy exits with a message, not silently and not by fighting for the microphone
        let socket = PathBuf::from("/tmp/molva-test.sock");
        let err = single_instance_guard(&socket, |_| Some(4242)).unwrap_err();
        assert_eq!(err.code, 7, "exit code \"the daemon is already running\"");
        assert!(err.message.contains("4242"), "{}", err.message);
        assert!(err.message.contains("molva-test.sock"), "{}", err.message);
    }

    #[test]
    fn the_first_instance_starts_normally() {
        let socket = PathBuf::from("/tmp/molva-test.sock");
        assert!(single_instance_guard(&socket, |_| None).is_ok());
    }

    #[test]
    fn auto_pull_stays_quiet_for_the_fake_engine_and_when_switched_off() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.model_path = directory.path().display().to_string();

        // The fake engine does not need weights: the network is not touched
        config.stt.engine = "fake".into();
        ensure_model_present(&config).unwrap();

        // A switched-off download with missing weights downloads nothing and does not fail here:
        // the engine build gives the error with the pull command, as before
        config.stt.engine = "whisper-cpp".into();
        config.stt.auto_pull = false;
        ensure_model_present(&config).unwrap();
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none(),
            "nothing was downloaded into the models directory"
        );
        let err = build_stt(&config).unwrap_err().to_string();
        assert!(err.contains("molva models pull"), "{err}");
    }

    #[test]
    fn the_fake_engine_is_available_and_missing_weights_say_how_to_get_them() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.stt.engine = "fake".into();
        match build_stt(&config) {
            Ok(engine) => assert_eq!(engine.id(), "fake"),
            Err(err) => panic!("the fake engine must build: {err}"),
        }
        config.stt.engine = "whisper-cpp".into();
        config.stt.model_path = directory.path().display().to_string();
        // Tests do not go to the network: without this the daemon would honestly download the weights into a temporary directory
        config.stt.auto_pull = false;
        let err = match build_stt(&config) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("the engine must not build without the weights file"),
        };
        assert!(err.contains("molva models pull"), "{err}");
    }

    #[test]
    fn journal_is_silent_in_no_record_mode_and_a_file_otherwise() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.journal.path = directory.path().join("journal.jsonl").display().to_string();
        config.privacy.no_record_mode = true;
        build_journal(&config).unwrap();
        assert!(!directory.path().join("journal.jsonl").exists());

        config.privacy.no_record_mode = false;
        build_journal(&config).unwrap();
        assert!(directory.path().join("journal.jsonl").exists());
    }

    #[test]
    fn llm_is_absent_unless_enabled() {
        let config = Config::default();
        assert!(build_llm(&config).is_none());
    }

    #[test]
    fn missing_dictionary_file_gives_an_empty_dictionary() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join("config.toml");
        let config = Config::default();
        let dictionary = build_dictionary(&config, &config_path);
        assert_eq!(dictionary.apply("привет").1, 0);
    }
}
