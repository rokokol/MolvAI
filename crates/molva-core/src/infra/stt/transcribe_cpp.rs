// SPDX-License-Identifier: MIT
//! Recognition through the `transcribe-cli` binary of transcribe.cpp
//!
//! whisper.cpp cannot load a GigaAM GGUF: the two families share neither the tensor layout
//! nor the decoder. This engine keeps the two runtimes apart by calling the CLI in a
//! separate process, so the whisper.cpp symbols that the rest of MolvAI links never meet
//! transcribe.cpp's own copy of ggml.
//!
//! The price is one process per request, which is why the model file and the binary are
//! fixed at construction and the audio goes through a temporary 16 kHz WAV.
//!
//! A cancel kills the process, and the call then ends with [`SttError::Cancelled`]. What the kill
//! reaches is that one process: a helper that the CLI started itself may keep the pipes open
//! after it dies. Such a call ends with an error once the wait for the output is over, never with
//! a truncated transcript, and the readers end when the last writer lets the pipes go

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use shared_child::SharedChild;
use tracing::debug;

use crate::domain::audio::{PcmAudio, TARGET_SAMPLE_RATE};
use crate::domain::cancel::CancelToken;
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};

/// Bytes that one stream of the CLI may print before the answer is refused
///
/// A bound is what keeps a runaway CLI from eating the memory of the machine, and it is set far
/// above the report of any real recording: a model that reports a token every forty milliseconds
/// prints about three megabytes an hour, so twenty hours of it fit twice over
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// How long the readers get to deliver what they read once the process is gone
const READER_TIMEOUT: Duration = Duration::from_secs(10);

/// The slowest recognition that the wait accepts, in seconds of process time per second of audio
///
/// The CLI recognizes an order of magnitude faster than the recording on a CPU, so this is a
/// hang detector and not a promise about speed: a run that lasts longer than this multiple is
/// dead, and the caller hears so instead of waiting for ever
const SLOWEST_RUN_FACTOR: f64 = 60.0;

/// The floor and the ceiling of that wait, in seconds
///
/// The floor covers the model load of a word of audio; the ceiling keeps the arithmetic honest
/// for a recording of many hours
const MIN_RUN_TIMEOUT_SECS: f64 = 300.0;
const MAX_RUN_TIMEOUT_SECS: f64 = 86_400.0;

/// What one reader brings back: the bytes, or the reason it stopped
///
type StreamResult = Result<Vec<u8>, String>;

/// Recognizer on transcribe.cpp: local, one process per request
#[derive(Debug)]
pub struct TranscribeCppEngine {
    model_path: PathBuf,
    model_name: String,
    /// Empty means `transcribe-cli` from PATH
    cli: String,
    /// Empty means the automatic choice of the CLI
    backend: String,
    threads: usize,
    /// The job a client may give up on: the token holds the process of the CLI while it runs
    cancel: Option<Arc<CancelToken>>,
}

impl TranscribeCppEngine {
    pub fn new(
        model_path: PathBuf,
        model_name: String,
        cli: String,
        backend: String,
        threads: usize,
    ) -> Self {
        Self {
            model_path,
            model_name,
            cli,
            backend,
            threads,
            cancel: None,
        }
    }

    fn program(&self) -> &str {
        let cli = self.cli.trim();
        if cli.is_empty() {
            "transcribe-cli"
        } else {
            cli
        }
    }

    fn write_wav(audio: &PcmAudio) -> Result<tempfile::NamedTempFile, SttError> {
        let file = tempfile::Builder::new()
            .prefix("molva-stt-")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| SttError::Inference(format!("cannot create a temporary WAV: {e}")))?;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: TARGET_SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::new(file.as_file(), spec)
            .map_err(|e| SttError::Inference(format!("cannot write the WAV header: {e}")))?;
        for sample in &audio.samples {
            writer
                .write_sample(*sample)
                .map_err(|e| SttError::Inference(format!("cannot write the WAV data: {e}")))?;
        }
        writer
            .finalize()
            .map_err(|e| SttError::Inference(format!("cannot finish the WAV file: {e}")))?;
        Ok(file)
    }

    /// Whether the client gave up before this call reached the process
    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
    }

    /// Register the process in the token of the job, if the job has one
    fn register(&self, child: Arc<SharedChild>) -> Option<Attachment> {
        self.cancel
            .as_ref()
            .map(|token| Attachment::new(Arc::clone(token), child))
    }

    /// How long one run may take before it counts as hung
    fn run_timeout(audio_secs: f32) -> Duration {
        Duration::from_secs_f64(
            (f64::from(audio_secs).max(0.0) * SLOWEST_RUN_FACTOR)
                .clamp(MIN_RUN_TIMEOUT_SECS, MAX_RUN_TIMEOUT_SECS),
        )
    }
}

/// The registration of one process in the token of the job it works for
///
/// It goes however the call ends, so a late cancel of a finished job finds nothing to kill and
/// cannot take the process of the next job
#[derive(Debug)]
struct Attachment {
    token: Arc<CancelToken>,
    child: Arc<SharedChild>,
}

impl Attachment {
    fn new(token: Arc<CancelToken>, child: Arc<SharedChild>) -> Self {
        token.attach(Arc::clone(&child));
        Self { token, child }
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.token.finish(&self.child);
    }
}

/// End a process that nothing waits for any more, and collect it
///
/// A killed process is collected here so that the call leaves no zombie behind; the wait is
/// short because the signal cannot be ignored
fn stop(child: &SharedChild) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Read one stream of the CLI into memory, refusing an answer above `limit` bytes
///
/// The bytes stay in memory and in the caller: the output carries recognized speech, and a file
/// on disk would keep it after the call
fn read_stream(mut stream: impl Read, limit: usize) -> StreamResult {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(data),
            Ok(read) => {
                if data.len() + read > limit {
                    return Err(format!(
                        "transcribe-cli printed more than {limit} bytes on one stream"
                    ));
                }
                data.extend_from_slice(&chunk[..read]);
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => {
                return Err(format!("cannot read the output of transcribe-cli: {err}"));
            }
        }
    }
}

/// Read one stream on a thread of its own
///
/// A pipe holds a few kilobytes: a writer that fills one while nobody reads it blocks, so one
/// thread that read the two streams in turn would wait for the first of them for ever
fn spawn_reader(stream: impl Read + Send + 'static) -> std::io::Result<Receiver<StreamResult>> {
    let (tx, rx): (Sender<StreamResult>, _) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("stt-output".into())
        .spawn(move || {
            // The receiver is gone when the call gave up: the reader has nothing to tell it then
            let _ = tx.send(read_stream(stream, MAX_STREAM_BYTES));
        })?;
    Ok(rx)
}

/// Collect what both readers read, waiting no longer than [`READER_TIMEOUT`] in all
fn collect_output(
    stdout: &Receiver<StreamResult>,
    stderr: &Receiver<StreamResult>,
) -> Result<(Vec<u8>, Vec<u8>), SttError> {
    let deadline = Instant::now() + READER_TIMEOUT;
    let take = |stream: &Receiver<StreamResult>, what: &str| {
        match stream
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(reason)) => Err(SttError::Inference(reason)),
        Err(_) => Err(SttError::Inference(format!(
            "transcribe-cli kept its {what} open after it exited: a process it started still holds it"
        ))),
    }
    };
    Ok((take(stdout, "output")?, take(stderr, "error output")?))
}

fn seconds_to_ms(value: &str) -> Option<u32> {
    let seconds: f64 = value.trim().parse().ok()?;
    Some((seconds * 1000.0).round().max(0.0) as u32)
}

/// One `[ 0.00 -> 2.50] text` line of the CLI output
fn parse_segment(line: &str) -> Option<Segment> {
    let body = line.trim_start().strip_prefix('[')?;
    let (start, rest) = body.split_once(" -> ")?;
    let (end, text) = rest.split_once(']')?;
    Some(Segment {
        start_ms: seconds_to_ms(start)?,
        end_ms: seconds_to_ms(end)?,
        text: text.trim().to_string(),
    })
}

/// One `[ 1.08 -> 1.12] p=0.000 token` line of the token timestamp block
fn parse_token(line: &str) -> Option<(u32, u32, String)> {
    let body = line.trim_start().strip_prefix('[')?;
    let (start, rest) = body.split_once(" -> ")?;
    let (end, rest) = rest.split_once(']')?;
    let rest = rest.trim_start().strip_prefix("p=")?;
    let (_, text) = rest.split_once(' ')?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some((seconds_to_ms(start)?, seconds_to_ms(end)?, text.to_string()))
}

/// A pause that starts a new segment when the model reports tokens rather than sentences
///
/// GigaAM puts a token every forty milliseconds and leaves tens of milliseconds between them, so
/// a pause of four hundred milliseconds happens in the middle of a phrase: the segments came out
/// shorter than a second and the text was cut at places where nobody paused. Seven hundred is the
/// pause the streaming segmenter already treats as the end of a phrase
const TOKEN_PAUSE_MS: u32 = 700;
/// A long stretch without a pause still becomes several segments, so a timecode stays useful
const TOKEN_SEGMENT_MS: u32 = 15_000;
/// A segment shorter than this joins its neighbour: a timecode under a second is no use to a reader
const TOKEN_MIN_SEGMENT_MS: u32 = 1_200;

/// Group tokens into segments at pauses and at a maximum length
///
/// GigaAM reports token timestamps only. A token carries a `▁` marker before a word, and the
/// marker is the only space signal: punctuation comes without one. A segment starts at a
/// pause, so a timecode points at the place in the recording where the words were said.
fn group_tokens(tokens: &[(u32, u32, String)]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut text = String::new();
    let mut start = 0;
    let mut end = 0;
    for (token_start, token_end, raw) in tokens {
        let word = raw.strip_prefix('▁').unwrap_or(raw);
        let new_word = raw.starts_with('▁');
        let pause = token_start.saturating_sub(end);
        let too_long = token_end.saturating_sub(start) > TOKEN_SEGMENT_MS;
        // A pause alone is not enough: GigaAM leaves more than half a second between words of a
        // slow phrase, so a cut there would leave the list full of fragments. The phrase is cut
        // once it holds something a reader can use, and its sound is long enough to seek to
        let long_enough = end.saturating_sub(start) >= TOKEN_MIN_SEGMENT_MS;
        if !text.is_empty() && (too_long || (pause > TOKEN_PAUSE_MS && long_enough)) {
            segments.push(Segment {
                start_ms: start,
                end_ms: end,
                text: text.trim().to_string(),
            });
            text.clear();
        }
        if text.is_empty() {
            start = *token_start;
        } else if new_word {
            text.push(' ');
        }
        text.push_str(word);
        end = *token_end;
    }
    if !text.trim().is_empty() {
        segments.push(Segment {
            start_ms: start,
            end_ms: end,
            text: text.trim().to_string(),
        });
    }
    merge_short_segments(segments)
}

/// Join a segment that is too short to carry a timecode into the one before it
///
/// The last segment absorbs its own shortness: there is nothing after it, and a fragment of one
/// word in the list is no better than a longer line that starts a little earlier
fn merge_short_segments(segments: Vec<Segment>) -> Vec<Segment> {
    let mut merged: Vec<Segment> = Vec::with_capacity(segments.len());
    for segment in segments {
        match merged.last_mut() {
            Some(previous)
                if previous.end_ms.saturating_sub(previous.start_ms) < TOKEN_MIN_SEGMENT_MS
                    || segment.end_ms.saturating_sub(segment.start_ms) < TOKEN_MIN_SEGMENT_MS =>
            {
                previous.end_ms = segment.end_ms;
                previous.text.push(' ');
                previous.text.push_str(segment.text.trim());
            }
            _ => merged.push(segment),
        }
    }
    merged
}

/// Read the parts of the CLI output that MolvAI needs
///
/// The CLI is an example driver and prints a report around the result: audio facts, the
/// model, the run status, timings. The `text:` line and the `segments:` block are the
/// contract used here; a model without sentence timestamps prints a `tokens:` block, and
/// its tokens are grouped into segments.
pub fn parse_output(stdout: &str) -> Result<Transcript, SttError> {
    let mut text: Option<String> = None;
    let mut detected_language = None;
    let mut segments = Vec::new();
    let mut tokens = Vec::new();
    let mut in_segments = false;
    let mut in_tokens = false;

    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("text: ") {
            text = Some(if value == "(empty)" {
                String::new()
            } else {
                value.to_string()
            });
            continue;
        }
        if let Some(value) = line.strip_prefix("detected-language: ") {
            detected_language = Some(value.trim().to_string());
            continue;
        }
        if line.starts_with("segments:") {
            in_segments = true;
            in_tokens = false;
            continue;
        }
        if line.starts_with("tokens:") {
            in_tokens = true;
            in_segments = false;
            continue;
        }
        if line.starts_with("words:") {
            in_segments = false;
            in_tokens = false;
            continue;
        }
        if in_segments {
            if let Some(segment) = parse_segment(line) {
                segments.push(segment);
            }
        } else if in_tokens {
            if let Some(token) = parse_token(line) {
                tokens.push(token);
            }
        }
    }
    if segments.is_empty() && !tokens.is_empty() {
        segments = group_tokens(&tokens);
    }

    let text = text.ok_or_else(|| {
        SttError::Inference("transcribe-cli printed no text line; check its version".into())
    })?;
    Ok(Transcript {
        text,
        segments,
        detected_language,
        no_speech_prob: None,
        cost_usd: None,
        billed_seconds: None,
    })
}

impl SttEngine for TranscribeCppEngine {
    // The trait ties the name to `&self` because an engine reads its own field; a literal here
    // cannot be declared static without changing the trait for that engine (clippy#11230)
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        "transcribe-cpp"
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }

    fn transcribe(
        &mut self,
        audio: &PcmAudio,
        options: &SttOptions,
    ) -> Result<Transcript, SttError> {
        if audio.samples.is_empty() {
            return Err(SttError::EmptyAudio);
        }
        if !Path::new(&self.model_path).is_file() {
            return Err(SttError::ModelMissing {
                path: self.model_path.display().to_string(),
                model: self.model_name.clone(),
            });
        }
        // The client gave up before this call reached the CLI: there is no process to stop, and
        // starting one to kill it at once would spend the model load for nothing
        if self.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        let ready = audio.to_16k();
        let wav = Self::write_wav(&ready)?;

        let mut command = Command::new(self.program());
        command
            .arg("--model")
            .arg(&self.model_path)
            .arg("--quiet")
            .arg("--timestamps")
            .arg(if options.timestamps { "auto" } else { "none" });
        if let LanguageHint::Fixed(code) = &options.language {
            command.arg("--language").arg(code);
        }
        let threads = options.threads.max(self.threads);
        if threads > 0 {
            command.arg("--threads").arg(threads.to_string());
        }
        if !self.backend.trim().is_empty() {
            command.arg("--backend").arg(self.backend.trim());
        }
        command
            .arg(wav.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started = Instant::now();
        let child = SharedChild::spawn(&mut command).map_err(|e| {
            SttError::Inference(format!(
                "cannot start {}: {e}. Install transcribe.cpp or set stt.transcribe_cli",
                self.program()
            ))
        })?;
        let child = Arc::new(child);
        // The streams are taken before the registration: reading them is what a cancel would
        // interrupt, and the pipes belong to the call from here on
        let stdout = child.take_stdout();
        let stderr = child.take_stderr();
        let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
            stop(&child);
            return Err(SttError::Inference(
                "transcribe-cli started without the output pipes it was given".into(),
            ));
        };
        // The registration goes before the readers: from here on a cancel kills the process,
        // and a cancel that arrived earlier has already set the flag that `attach` reads
        let _registration = self.register(Arc::clone(&child));
        let readers = match (spawn_reader(stdout), spawn_reader(stderr)) {
            (Ok(out), Ok(err)) => Ok((out, err)),
            (out, err) => {
                drop(out);
                drop(err);
                Err(SttError::Inference(
                    "cannot start a thread that reads the output of transcribe-cli".into(),
                ))
            }
        };
        let (stdout, stderr) = match readers {
            Ok(readers) => readers,
            Err(err) => {
                stop(&child);
                return Err(err);
            }
        };
        let timeout = Self::run_timeout(ready.duration_secs());
        let status = match child.wait_deadline(Instant::now() + timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                stop(&child);
                return Err(SttError::Inference(format!(
                    "transcribe-cli did not finish in {} s and was stopped",
                    timeout.as_secs()
                )));
            }
            Err(e) => {
                stop(&child);
                return Err(SttError::Inference(format!(
                    "cannot wait for {}: {e}",
                    self.program()
                )));
            }
        };
        // The process died because the client gave up, and its status says so; the reason of
        // the failure is the cancel, not a broken install. The readers end at the end of the
        // pipe on their own, and nothing of what they read belongs to anybody
        if self.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        let (out, err) = collect_output(&stdout, &stderr)?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&err);
            let reason = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("no message");
            return Err(SttError::Inference(format!(
                "transcribe-cli exited with {status}: {reason}"
            )));
        }
        let stdout = String::from_utf8_lossy(&out);
        let mut transcript = parse_output(&stdout)?;
        if !options.timestamps {
            transcript.segments.clear();
        }
        debug!(
            model = %self.model_name,
            audio_secs = ready.duration_secs(),
            stt_ms = started.elapsed().as_millis() as u64,
            segments = transcript.segments.len(),
            "transcribe.cpp recognition finished"
        );
        Ok(transcript)
    }

    /// The job a client may give up on: the token holds the process while it runs, and a cancel
    /// then kills the CLI instead of waiting for the model to finish
    fn set_cancel(&mut self, cancel: Option<Arc<CancelToken>>) {
        self.cancel = cancel;
    }

    fn unload(&mut self) {
        // The CLI process exits after every request: there is no model in this process
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_segments() {
        let out = "\
audio: /tmp/a.wav
  samples:    16000
text: привет мир
detected-language: ru
segments: 2
  [   0.00 ->    1.25] привет
  [   1.25 ->    2.50] мир
";
        let transcript = parse_output(out).expect("the report is readable");
        assert_eq!(transcript.text, "привет мир");
        assert_eq!(transcript.detected_language.as_deref(), Some("ru"));
        assert_eq!(
            transcript.segments,
            vec![
                Segment {
                    start_ms: 0,
                    end_ms: 1250,
                    text: "привет".into()
                },
                Segment {
                    start_ms: 1250,
                    end_ms: 2500,
                    text: "мир".into()
                }
            ]
        );
    }

    #[test]
    fn token_timestamps_become_segments_at_pauses() {
        let out = "\
text: Привет! Как дела
tokens: 5
  [   1.08 ->    1.12] p=0.000 ▁При
  [   1.24 ->    1.28] p=0.000 вет
  [   2.60 ->    2.64] p=0.000 !
  [   3.60 ->    3.64] p=0.000 ▁Как
  [   4.90 ->    4.94] p=0.000 ▁дела
";
        let transcript = parse_output(out).expect("the report is readable");
        assert_eq!(transcript.segments.len(), 2, "{:?}", transcript.segments);
        assert_eq!(transcript.segments[0].text, "Привет!");
        assert_eq!(transcript.segments[0].start_ms, 1080);
        assert_eq!(transcript.segments[1].text, "Как дела");
        assert_eq!(transcript.segments[1].start_ms, 3600);
    }

    #[test]
    fn a_segment_shorter_than_a_second_joins_its_neighbour() {
        // A pause of forty milliseconds inside the phrase does not cut it, and a tail of two
        // tokens is too short to carry a timecode of its own
        let out = "\
text: раз два три
tokens: 6
  [   0.40 ->    0.44] p=0.000 ▁раз
  [   0.52 ->    0.56] p=0.000 ▁два
  [   0.64 ->    0.68] p=0.000 ▁три.
  [   1.80 ->    1.84] p=0.000 ▁Ну
  [   1.92 ->    1.96] p=0.000 ▁и
  [   2.04 ->    2.08] p=0.000 ▁всё
";
        let transcript = parse_output(out).expect("the report is readable");
        assert_eq!(transcript.segments.len(), 1, "{:?}", transcript.segments);
        assert_eq!(transcript.segments[0].text, "раз два три. Ну и всё");
        assert_eq!(transcript.segments[0].start_ms, 400);
        assert_eq!(transcript.segments[0].end_ms, 2080);
    }

    #[test]
    fn empty_text_is_not_a_missing_text_line() {
        let transcript = parse_output("text: (empty)\n").expect("empty speech is a result");
        assert_eq!(transcript.text, "");
    }

    #[test]
    fn a_report_without_a_text_line_is_an_error() {
        let error = parse_output("audio: /tmp/a.wav\n").expect_err("nothing was recognized");
        assert!(error.to_string().contains("text line"), "{error}");
    }

    #[test]
    fn an_empty_cli_path_falls_back_to_path_lookup() {
        let engine = TranscribeCppEngine::new(
            PathBuf::from("/tmp/model.gguf"),
            "gigaam".into(),
            String::new(),
            String::new(),
            0,
        );
        assert_eq!(engine.program(), "transcribe-cli");
        assert_eq!(engine.id(), "transcribe-cpp");
    }

    #[test]
    fn a_stream_above_the_limit_is_an_error_and_not_a_truncated_answer() {
        // The limit of the engine is far above any real report, so the reader is asked here with
        // one that a test can reach: the answer is refused, never cut short
        let data = vec![b'a'; 64 * 1024];

        let error = read_stream(&data[..], 1024).expect_err("the answer is refused");

        assert!(error.contains("bytes"), "{error}");
        assert_eq!(
            read_stream(&data[..1024], 1024)
                .expect("the answer fits")
                .len(),
            1024
        );
    }

    /// The engine as the pipeline builds it, and the stand-ins for the CLI that its tests run
    ///
    /// The stand-in is a shell script that ignores the arguments: the engine talks to a process,
    /// and what the process does is the whole of what these tests need from it
    #[cfg(unix)]
    mod process {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// The longest a test waits for an answer that must arrive at once
        const ANSWER: Duration = Duration::from_secs(20);

        /// Write a stand-in for `transcribe-cli` into `dir`
        fn cli(dir: &Path, name: &str, body: &str) -> PathBuf {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}")).expect("the stand-in is written");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("the stand-in is executable");
            path
        }

        /// An engine over that stand-in, with a model file that the engine only checks for
        fn engine(cli: &Path) -> TranscribeCppEngine {
            let model = cli.with_file_name("model.gguf");
            std::fs::write(&model, b"not a model").expect("the model file is in place");
            TranscribeCppEngine::new(
                model,
                "test-model".into(),
                cli.display().to_string(),
                String::new(),
                0,
            )
        }

        fn speech() -> PcmAudio {
            PcmAudio::new(vec![0.1; 1600], 16_000)
        }

        /// Run one recognition on a thread of its own, so a call that never returns fails the
        /// test instead of hanging the suite
        fn transcribe_within(
            mut engine: Box<dyn SttEngine>,
            wait: Duration,
        ) -> (Result<Transcript, SttError>, Duration) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = engine.transcribe(&speech(), &SttOptions::default());
                let _ = tx.send((result, started.elapsed()));
            });
            rx.recv_timeout(wait).expect("the call returned in time")
        }

        /// Run one recognition and cancel it the moment its process is registered
        fn transcribe_cancelled(
            mut engine: Box<dyn SttEngine>,
            token: &Arc<CancelToken>,
        ) -> (Result<Transcript, SttError>, Duration) {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let started = Instant::now();
                let result = engine.transcribe(&speech(), &SttOptions::default());
                let _ = tx.send((result, started.elapsed()));
            });
            assert!(token.wait_attached(ANSWER), "the CLI process never started");
            token.cancel();
            rx.recv_timeout(ANSWER)
                .expect("the cancel reached the process")
        }

        #[test]
        fn a_cancel_through_the_trait_stops_the_process() {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let script = cli(dir.path(), "slow.sh", "sleep 300\n");
            let token = Arc::new(CancelToken::new());
            // The call goes through the trait object, as the pipeline makes it: an inherent
            // method of the engine would never be reached from here
            let mut engine: Box<dyn SttEngine> = Box::new(engine(&script));
            engine.set_cancel(Some(Arc::clone(&token)));

            let (result, elapsed) = transcribe_cancelled(engine, &token);

            assert!(matches!(result, Err(SttError::Cancelled)), "{result:?}");
            assert!(elapsed < ANSWER, "the call waited {elapsed:?}");
        }

        #[test]
        fn a_cancel_before_the_start_never_runs_the_cli() {
            let dir = tempfile::tempdir().expect("a temporary directory");
            // A program that cannot start at all: a call that answers `Cancelled` therefore never
            // tried to start a process, because the attempt would answer with the reason instead
            let unrunnable = dir.path().join("not-a-program");
            std::fs::write(&unrunnable, b"text: not a program\n").expect("the file is written");
            std::fs::set_permissions(&unrunnable, std::fs::Permissions::from_mode(0o644))
                .expect("the file is not executable");
            let token = Arc::new(CancelToken::new());
            token.cancel();
            let mut engine: Box<dyn SttEngine> = Box::new(engine(&unrunnable));
            engine.set_cancel(Some(Arc::clone(&token)));

            let (result, elapsed) = transcribe_within(engine, Duration::from_secs(3));

            assert!(
                matches!(result, Err(SttError::Cancelled)),
                "the call started a process after the client gave up: {result:?}"
            );
            assert!(
                elapsed < Duration::from_secs(1),
                "the call waited {elapsed:?}"
            );
        }

        #[test]
        fn a_finished_job_registers_nothing_and_leaves_the_next_process_alone() {
            let dir = tempfile::tempdir().expect("a temporary directory");
            let fast = cli(dir.path(), "fast.sh", "echo 'text: привет'\n");
            let token = Arc::new(CancelToken::new());
            let mut first: Box<dyn SttEngine> = Box::new(engine(&fast));
            first.set_cancel(Some(Arc::clone(&token)));

            let (result, _) = transcribe_within(first, ANSWER);

            assert_eq!(result.expect("the answer is read").text, "привет");
            assert!(
                !token.is_attached(),
                "the finished job left its process registered"
            );
            assert!(!token.is_cancelled());

            // The next call of the same job is not touched by the process that already ended:
            // its own process starts, and the cancel kills that one
            let slow = cli(dir.path(), "slow.sh", "sleep 300\n");
            let mut second: Box<dyn SttEngine> = Box::new(engine(&slow));
            second.set_cancel(Some(Arc::clone(&token)));
            let (result, _) = transcribe_cancelled(second, &token);
            assert!(matches!(result, Err(SttError::Cancelled)), "{result:?}");
        }

        #[test]
        fn a_large_answer_on_both_streams_is_read_without_a_deadlock() {
            let dir = tempfile::tempdir().expect("a temporary directory");
            // More than the pipe buffer of either stream: a reader that took the streams in turn
            // would wait for the first of them while the CLI waits for someone to read the other
            let script = cli(
                dir.path(),
                "noisy.sh",
                "(yes aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa | head -c 1048576)\n\
                 (yes bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb | head -c 1048576) >&2\n\
                 echo\n\
                 echo 'text: привет'\n",
            );
            let engine: Box<dyn SttEngine> = Box::new(engine(&script));

            let (result, _) = transcribe_within(engine, ANSWER);

            assert_eq!(result.expect("the answer is read").text, "привет");
        }
    }
}
