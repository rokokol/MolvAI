// SPDX-License-Identifier: MIT
//! The journal of a real `molva` binary, encrypted: what reaches the disk, what the history
//! reads back, and what happens when the key is gone
//!
//! Every test here runs the binary against a settings file of its own in a temporary directory.
//! The recognizer is the fake engine, so no weights and no microphone are involved, and the
//! environment is cut off from the journal of the person who runs the suite

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The text the fake engine answers with: it must never appear on the disk
const FAKE_TEXT: &str = "тестовая расшифровка";

struct Workspace {
    directory: tempfile::TempDir,
}

impl Workspace {
    fn new(encrypt: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = Self { directory };
        let journal = toml_path(&workspace.path().join("journal.jsonl"));
        let key = toml_path(&workspace.path().join("journal.key"));
        let text = format!(
            "[stt]\nengine = \"fake\"\nauto_pull = false\nmodel = \"small\"\nlanguage = \"ru\"\n\n\
             [llm]\nenabled = false\n\n\
             [privacy]\nsend_to_llm = false\ncloud_consent = false\nno_record_mode = false\n\n\
             [mcp]\nenabled = false\n\n\
             [journal]\npath = \"{journal}\"\nenabled = true\ninclude_text = true\n\
             encrypt = {encrypt}\nkey_source = \"file\"\nkey_path = \"{key}\"\n"
        );
        std::fs::write(workspace.config(), text).unwrap();
        workspace
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn config(&self) -> PathBuf {
        self.path().join("config.toml")
    }

    fn journal(&self) -> PathBuf {
        self.path().join("journal.jsonl")
    }

    fn key(&self) -> PathBuf {
        self.path().join("journal.key")
    }

    /// The binary under test, with the environment of this workspace and nothing else
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_molva"));
        command
            .arg("--config")
            .arg(self.config())
            .arg("--socket")
            .arg(self.path().join("unused.sock"))
            .env("HOME", self.path())
            .env("XDG_CONFIG_HOME", self.path().join("config"))
            .env("XDG_DATA_HOME", self.path().join("data"))
            .env("XDG_RUNTIME_DIR", self.path().join("run"))
            .env("MOLVA_DATA_DIR", self.path().join("data"))
            .env_remove("MOLVA_RUNTIME_CONFIG")
            .env_remove("MOLVA_STT");
        command
    }

    /// One dictation through the file pipeline, which writes the journal
    fn transcribe(&self) -> Output {
        let wav = self.path().join("phrase.wav");
        write_wav(&wav);
        self.command()
            .args(["transcribe", "--engine", "fake", "--no-llm", "--no-inject"])
            .arg(&wav)
            .output()
            .unwrap()
    }

    /// The history as the command prints it, with the same settings
    fn history(&self) -> Output {
        self.command()
            .args(["history", "--format", "json"])
            .output()
            .unwrap()
    }
}

fn toml_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}

fn write_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for index in 0..16_000u32 {
        let value = ((index as f32 * 0.05).sin() * 8_000.0) as i16;
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_encrypted_journal_keeps_the_phrase_off_the_disk_and_reads_it_back() {
    let workspace = Workspace::new(true);

    let output = workspace.transcribe();
    assert!(output.status.success(), "{}", stderr(&output));

    let disk = std::fs::read_to_string(workspace.journal()).unwrap();
    assert!(
        !disk.contains(FAKE_TEXT),
        "the phrase reached the disk in plaintext: {disk}"
    );
    assert!(
        disk.contains("enc1:"),
        "the journal holds no ciphertext: {disk}"
    );
    assert!(workspace.key().is_file(), "the key was not created");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(workspace.key())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the key is readable by other users");
    }

    let history = workspace.history();
    assert!(history.status.success(), "{}", stderr(&history));
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    let text = entries[0]["text_raw"].as_str().unwrap_or_default();
    assert_eq!(text, FAKE_TEXT, "the history does not read the phrase back");
}

#[test]
fn a_missing_key_over_ciphertext_is_refused_and_creates_no_new_key() {
    let workspace = Workspace::new(true);
    let output = workspace.transcribe();
    assert!(output.status.success(), "{}", stderr(&output));

    // The key is lost: the journal still holds the ciphertext, and a new key would make those
    // entries unreadable for good
    let kept = workspace.path().join("kept.key");
    std::fs::rename(workspace.key(), &kept).unwrap();

    let history = workspace.history();
    assert!(
        !history.status.success(),
        "a journal with ciphertext and no key was read as if nothing were wrong"
    );
    let message = stderr(&history);
    assert!(
        message.contains("key"),
        "the refusal does not name the key: {message}"
    );
    assert!(
        !workspace.key().exists(),
        "a new key replaced the lost one, and the old entries are gone"
    );

    // With the key back, the phrase is readable again
    std::fs::rename(&kept, workspace.key()).unwrap();
    let history = workspace.history();
    assert!(history.status.success(), "{}", stderr(&history));
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(entries[0]["text_raw"].as_str().unwrap_or_default(), FAKE_TEXT);
}

#[test]
fn without_encryption_the_phrase_is_on_the_disk_and_no_key_appears() {
    let workspace = Workspace::new(false);
    let output = workspace.transcribe();
    assert!(output.status.success(), "{}", stderr(&output));

    let disk = std::fs::read_to_string(workspace.journal()).unwrap();
    assert!(disk.contains(FAKE_TEXT), "the plain journal lost the phrase");
    assert!(
        !workspace.key().exists(),
        "a key was created for a journal that is not encrypted"
    );

    let history = workspace.history();
    assert!(history.status.success(), "{}", stderr(&history));
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(entries[0]["text_raw"].as_str().unwrap_or_default(), FAKE_TEXT);
}

#[test]
fn a_disabled_journal_writes_nothing_at_all() {
    let workspace = Workspace::new(true);
    let text = std::fs::read_to_string(workspace.config())
        .unwrap()
        .replace("enabled = true", "enabled = false");
    std::fs::write(workspace.config(), text).unwrap();

    let output = workspace.transcribe();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        !workspace.journal().exists(),
        "a disabled journal was created"
    );
    assert!(
        !workspace.key().exists(),
        "a disabled journal created a key"
    );
}
