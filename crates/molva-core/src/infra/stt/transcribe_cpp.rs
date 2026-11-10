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

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use tracing::debug;

use crate::domain::audio::{PcmAudio, TARGET_SAMPLE_RATE};
use crate::domain::stt::{LanguageHint, Segment, SttEngine, SttError, SttOptions, Transcript};

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
    cancel: Option<std::sync::Arc<crate::domain::cancel::CancelToken>>,
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

    /// The job a client may give up on, so a cancel reaches the running process
    pub fn set_cancel(&mut self, cancel: std::sync::Arc<crate::domain::cancel::CancelToken>) {
        self.cancel = Some(cancel);
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
    })
}

impl SttEngine for TranscribeCppEngine {
    fn id(&self) -> &'static str {
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
        // Spawn first, then wait: the process identifier is what a cancel needs, and
        // `output()` would hold it inside the call until the model finishes
        let child = command.spawn().map_err(|e| {
            SttError::Inference(format!(
                "cannot start {}: {e}. Install transcribe.cpp or set stt.transcribe_cli",
                self.program()
            ))
        })?;
        if let Some(cancel) = &self.cancel {
            cancel.attach(child.id());
        }
        let output = child
            .wait_with_output()
            .map_err(|e| SttError::Inference(format!("cannot wait for {}: {e}", self.program())))?;
        if let Some(cancel) = &self.cancel {
            cancel.detach();
            // The process died because the client gave up, and its status says so; the reason
            // of the failure is the cancel, not a broken install
            if cancel.is_cancelled() {
                return Err(SttError::Cancelled);
            }
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("no message");
            return Err(SttError::Inference(format!(
                "transcribe-cli exited with {}: {reason}",
                output.status
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
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
}
