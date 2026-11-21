// SPDX-License-Identifier: MIT

use whisper_rs::{WhisperVadContext, WhisperVadContextParams};

pub(crate) const FRAME_SAMPLES: usize = 512;
const BLOCK_FRAMES: usize = 512;
const CONTEXT_FRAMES: usize = 32;
const SPEECH_THRESHOLD: f32 = 0.5;
const MODEL: &[u8] = include_bytes!("ggml-silero-v5.1.2.bin");

/// Classify 16 kHz mono audio without a second copy of the recording
pub(crate) fn classify(
    samples: &[f32],
    cancelled: impl Fn() -> bool,
) -> Result<Option<Vec<bool>>, String> {
    if samples.is_empty() || cancelled() {
        return Ok(None);
    }
    // The VAD reads its weights from a path, so the embedded copy becomes a file for the
    // length of this call
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = directory.path().join("silero.bin");
    std::fs::write(&path, MODEL).map_err(|error| error.to_string())?;
    let mut params = WhisperVadContextParams::default();
    params.set_use_gpu(false);
    params.set_n_threads(1);
    let path = path.to_str().ok_or("Silero model path is not UTF-8")?;
    let mut vad = WhisperVadContext::new(path, params).map_err(|error| error.to_string())?;
    let block = BLOCK_FRAMES * FRAME_SAMPLES;
    let context = CONTEXT_FRAMES * FRAME_SAMPLES;
    // One pass over the samples: a decoded hour would cost 220 MB as a second copy
    let mut speech = Vec::with_capacity(samples.len().div_ceil(FRAME_SAMPLES));
    for start in (0..samples.len()).step_by(block) {
        if cancelled() {
            return Ok(None);
        }
        // whisper.cpp resets the model state on each call. Feed preceding audio to warm it up
        let from = start.saturating_sub(context);
        let end = start.saturating_add(block).min(samples.len());
        let input = &samples[from..end];
        vad.detect_speech(input)
            .map_err(|error| error.to_string())?;
        let probabilities = vad.probabilities();
        if probabilities.len() != input.len().div_ceil(FRAME_SAMPLES) {
            return Err("Unexpected Silero frame count".into());
        }
        speech.extend(
            probabilities[(start - from) / FRAME_SAMPLES..]
                .iter()
                .map(|&probability| probability >= SPEECH_THRESHOLD),
        );
    }
    // Do not treat a whole file as one confirmed pause when the model detects no speech
    Ok(speech.iter().any(|&voiced| voiced).then_some(speech))
}
