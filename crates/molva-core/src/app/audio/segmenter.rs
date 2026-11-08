// SPDX-License-Identifier: MIT
//! Cutting of a recording that is still running into chunks for recognition
//!
//! There is no reason to recognize a whole phrase after the key release: while the person
//! speaks, the processor is idle. The segmenter collects the stream and returns a ready chunk
//! as soon as enough sound has come in and the person has made a pause. Such a chunk goes to
//! whisper during the speech, and after the release only the tail remains to recognize
//!
//! A cut in the middle of a syllable is not allowed: from a fragment of a word, whisper
//! confidently prints a wrong word. So the boundary is searched at the middle of a pause, not by
//! a timer, and the pause that counts is `stt.chunk_pause_ms`, 700 ms by default
//!
//! [`DEFAULT_MAX_CHUNK_SECS`] is where the wait for that pause costs more than a less ideal
//! boundary: after ten seconds a pause of [`LATE_PAUSE_MS`] is enough. There is no forced cut. A
//! chunk that meets no acceptable pause keeps growing, so a person who speaks without pauses pays
//! for it with memory and a late first hypothesis. Neighbouring chunks overlap by
//! [`SegmenterConfig::overlap_ms`], so the sound exactly at the boundary falls into both
//!
//! The structure is pure: no time, no threads, no I/O. It takes only samples as input and
//! returns chunks as output, so synthetic signals test it without a microphone and without a
//! model

use crate::domain::audio::PcmAudio;

/// A chunk is never shorter than this: the segmenter ignores a pause before this length
///
/// Three seconds is a measurement, not a taste. On the `small` model and a CPU, recognition
/// runs at about the speed of speech, so the chunk length decides everything. With five seconds
/// the queue does not clear before the key release (12.6 → 12.2 s of waiting, so there is no
/// gain). With three seconds the wait falls by half (12.6 → 6.8 s). With two seconds it grows
/// again (9.6 s), because each extra chunk pays for its own encoder window padding
pub const DEFAULT_TARGET_CHUNK_SECS: f32 = 3.0;

/// Length after which the pause that cuts a chunk relaxes to [`LATE_PAUSE_MS`]
pub const DEFAULT_MAX_CHUNK_SECS: f32 = 10.0;

/// Pause that cuts a chunk once it is longer than [`DEFAULT_MAX_CHUNK_SECS`]
///
/// Ten seconds of one chunk is a long wait for a boundary, and a long sentence may hold no pause
/// of 700 ms. A pause of 200 ms arrives soon and still leaves silence on both sides of the cut
pub const LATE_PAUSE_MS: u32 = 200;

/// Overlap of neighbouring chunks: the sound at the boundary falls into both chunks
///
/// The overlap is also what whisper echoes: the next chunk holds sound the previous one already
/// had, so the last word comes back at the start of the next chunk. The daemon drops that echo by
/// text, see [`crate::app::daemon::chunked::strip_repeated_head`]
pub const DEFAULT_OVERLAP_MS: u32 = 150;

/// Pause at which a chunk is cut, when the settings do not say otherwise
pub const DEFAULT_CHUNK_PAUSE_MS: u32 = 700;

/// Window for the level analysis: as in [`crate::app::audio::trim`], 20 ms
const WINDOW_MS: u32 = 20;

/// Level that counts as absolute silence: the logarithm of zero is not defined
const SILENCE_FLOOR_DB: f32 = -120.0;

/// Cutting settings
#[derive(Debug, Clone, PartialEq)]
pub struct SegmenterConfig {
    /// Sample rate of the stream; chunks are returned at the same rate
    pub sample_rate: u32,
    /// A chunk is not returned before this length, even if a pause has already occurred
    pub target_chunk_secs: f32,
    /// After this length the pause that cuts a chunk relaxes to [`LATE_PAUSE_MS`]. The chunk is
    /// still not cut without a pause
    pub max_chunk_secs: f32,
    /// A pause shorter than this does not count as a boundary before that length
    pub min_pause_ms: u32,
    /// Silence threshold in dBFS, as `audio.silence_threshold_db`
    pub silence_threshold_db: f32,
    /// Overlap of neighbouring chunks
    pub overlap_ms: u32,
}

impl SegmenterConfig {
    /// Default settings for a stream rate: the pause and the threshold come from the audio config
    pub fn new(sample_rate: u32, min_pause_ms: u32, silence_threshold_db: f32) -> Self {
        Self {
            sample_rate,
            target_chunk_secs: DEFAULT_TARGET_CHUNK_SECS,
            max_chunk_secs: DEFAULT_MAX_CHUNK_SECS,
            min_pause_ms,
            silence_threshold_db,
            overlap_ms: DEFAULT_OVERLAP_MS,
        }
    }
}

/// A ready chunk of the recording
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub audio: PcmAudio,
    /// Sequence number of the chunk in the phrase, starting from zero
    pub index: usize,
    /// Offset of the chunk start from the start of the recording
    pub start_ms: u32,
}

impl Chunk {
    pub fn duration_secs(&self) -> f32 {
        self.audio.duration_secs()
    }
}

/// Cutter of a stream into chunks
#[derive(Debug)]
pub struct Segmenter {
    config: SegmenterConfig,
    /// Samples not yet returned; it starts with the overlap of the previous chunk
    buffer: Vec<f32>,
    /// Level of each whole window of the buffer in dBFS
    windows: Vec<f32>,
    /// Absolute index of `buffer[0]` from the start of the recording; `start_ms` is computed
    /// from it
    offset: usize,
    /// How many chunks are already returned
    emitted: usize,
}

impl Segmenter {
    pub fn new(config: SegmenterConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            windows: Vec::new(),
            offset: 0,
            emitted: 0,
        }
    }

    pub fn config(&self) -> &SegmenterConfig {
        &self.config
    }

    /// How many chunks are returned since the start of the recording
    pub fn emitted(&self) -> usize {
        self.emitted
    }

    /// Add fresh samples and take everything that has matured into a chunk
    ///
    /// One call can return several chunks: if the stream came as a big portion, it may hold
    /// several boundaries at once. Only a pause makes a chunk, so a stream without pauses
    /// returns nothing and keeps the sound until the release
    pub fn push(&mut self, samples: &[f32]) -> Vec<Chunk> {
        self.buffer.extend_from_slice(samples);
        let mut out = Vec::new();
        if self.config.sample_rate == 0 {
            // Without a sample rate there is no duration: nothing to cut by, and all goes to
            // the tail
            return out;
        }
        loop {
            self.analyse();
            if self.buffer.len() < self.target_samples() {
                break;
            }
            let Some(cut) = self.pause_cut(self.pause_required()) else {
                break;
            };
            if let Some(chunk) = self.take(cut) {
                out.push(chunk);
            }
        }
        out
    }

    /// Tail of the recording: everything that remains in the buffer and was not returned
    ///
    /// `None` if the remainder has no window louder than the threshold: there is no reason to
    /// recognize silence
    pub fn finish(&mut self) -> Option<Chunk> {
        self.analyse();
        let cut = self.buffer.len();
        if cut == 0 {
            return None;
        }
        self.take(cut)
    }

    /// Length of the analysis window in samples; for a zero rate it is one sample
    fn window_len(&self) -> usize {
        ((u64::from(self.config.sample_rate) * u64::from(WINDOW_MS) / 1000) as usize).max(1)
    }

    fn samples_for_secs(&self, secs: f32) -> usize {
        (self.config.sample_rate as f32 * secs.max(0.0)) as usize
    }

    fn samples_for_ms(&self, ms: u32) -> usize {
        (u64::from(self.config.sample_rate) * u64::from(ms) / 1000) as usize
    }

    fn target_samples(&self) -> usize {
        self.samples_for_secs(self.config.target_chunk_secs).max(1)
    }

    fn max_samples(&self) -> usize {
        self.samples_for_secs(self.config.max_chunk_secs)
            .max(self.target_samples())
    }

    /// Compute the level for the whole windows that appeared after the previous call
    fn analyse(&mut self) {
        let window = self.window_len();
        while (self.windows.len() + 1) * window <= self.buffer.len() {
            let from = self.windows.len() * window;
            let level = amplitude_to_db(rms(&self.buffer[from..from + window]));
            self.windows.push(level);
        }
    }

    fn is_silent_window(&self, index: usize) -> bool {
        self.windows[index] < self.config.silence_threshold_db
    }

    /// Pause that a chunk of the current length waits for
    ///
    /// Before the limit the person still has time, so the boundary needs the configured pause.
    /// After it, the wait costs more than a boundary that is not ideal, so [`LATE_PAUSE_MS`] is
    /// enough. A configured pause shorter than that stays shorter: this rule relaxes the setting
    /// and never tightens it
    ///
    /// The length decides, not the place of the pause: a chunk that has passed the limit takes
    /// the first pause the late rule accepts, even if that pause began before the limit. Waiting
    /// for the next pause would make the chunk longer for no gain
    fn pause_required(&self) -> u32 {
        if self.buffer.len() >= self.max_samples() {
            LATE_PAUSE_MS.min(self.config.min_pause_ms)
        } else {
            self.config.min_pause_ms
        }
    }

    /// Boundary at the middle of the first pause of at least `min_pause_ms` after `target`
    ///
    /// A pause at the end of the buffer counts the same as a finished pause. To wait until the
    /// person speaks again would waste the very time that the design exists to save
    fn pause_cut(&self, min_pause_ms: u32) -> Option<usize> {
        let window = self.window_len();
        let min_run = (min_pause_ms as usize / WINDOW_MS as usize).max(1);
        let target = self.target_samples();

        let mut run_start: Option<usize> = None;
        for index in 0..=self.windows.len() {
            let silent = index < self.windows.len() && self.is_silent_window(index);
            if silent {
                run_start.get_or_insert(index);
                continue;
            }
            let Some(start) = run_start.take() else {
                continue;
            };
            if index - start < min_run {
                continue;
            }
            let cut = usize::midpoint(start, index) * window;
            if cut >= target {
                return Some(cut.min(self.buffer.len()));
            }
        }
        None
    }

    /// Cut off a chunk at the boundary `cut` and keep the overlap for the next one
    ///
    /// `None` means that the chunk had no window louder than the threshold. Such a chunk does
    /// not go to whisper, or the model would print a subtitle tail on the silence
    fn take(&mut self, cut: usize) -> Option<Chunk> {
        let window = self.window_len();
        let cut = cut.clamp(1, self.buffer.len());
        let voiced = self
            .windows
            .iter()
            .take(cut / window)
            .any(|level| *level >= self.config.silence_threshold_db);

        let samples = self.buffer[..cut].to_vec();
        let start_ms = self.ms_of(self.offset);

        // The overlap stays for the next chunk; `max(1)` guarantees forward progress
        let keep_from = cut
            .saturating_sub(self.samples_for_ms(self.config.overlap_ms))
            .max(1)
            .min(cut);
        self.buffer.drain(..keep_from);
        self.offset += keep_from;
        // The windows were counted from the old buffer start: the alignment broke, so count again
        self.windows.clear();
        self.analyse();

        if !voiced {
            return None;
        }
        let index = self.emitted;
        self.emitted += 1;
        Some(Chunk {
            audio: PcmAudio::new(samples, self.config.sample_rate),
            index,
            start_ms,
        })
    }

    fn ms_of(&self, samples: usize) -> u32 {
        if self.config.sample_rate == 0 {
            return 0;
        }
        u32::try_from(samples as u64 * 1000 / u64::from(self.config.sample_rate))
            .unwrap_or(u32::MAX)
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

fn amplitude_to_db(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return SILENCE_FLOOR_DB;
    }
    (20.0 * amplitude.log10()).max(SILENCE_FLOOR_DB)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    /// Short thresholds: a test must not generate twelve seconds of signal per check
    fn config() -> SegmenterConfig {
        SegmenterConfig {
            sample_rate: RATE,
            target_chunk_secs: 0.5,
            max_chunk_secs: 1.5,
            min_pause_ms: 200,
            silence_threshold_db: -45.0,
            overlap_ms: 50,
        }
    }

    /// Settings with a configured pause well above [`LATE_PAUSE_MS`]: the two rules separate
    fn config_wide_pause() -> SegmenterConfig {
        SegmenterConfig {
            min_pause_ms: 700,
            max_chunk_secs: 2.5,
            ..config()
        }
    }

    fn tone(ms: u32) -> Vec<f32> {
        tone_at(ms, 0.5)
    }

    fn tone_at(ms: u32, amplitude: f32) -> Vec<f32> {
        let n = (u64::from(RATE) * u64::from(ms) / 1000) as usize;
        (0..n).map(|i| amplitude * (i as f32 * 0.3).sin()).collect()
    }

    fn silence(ms: u32) -> Vec<f32> {
        vec![0.0; (u64::from(RATE) * u64::from(ms) / 1000) as usize]
    }

    fn ms_of(chunk: &Chunk) -> u32 {
        (chunk.audio.samples.len() as u64 * 1000 / u64::from(RATE)) as u32
    }

    #[test]
    fn a_pause_between_two_tones_becomes_the_boundary() {
        let mut segmenter = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(
            chunks.len(),
            1,
            "the pause did not become a boundary: {chunks:?}"
        );
        // The boundary is the middle of the pause: 600 ms of tone plus half of 400 ms of silence
        let cut = ms_of(&chunks[0]);
        assert!(
            (750..=850).contains(&cut),
            "the cut must be at the middle of the pause, but it came at {cut} ms"
        );
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].start_ms, 0);
    }

    #[test]
    fn a_chunk_without_any_pause_keeps_growing() {
        let mut segmenter = Segmenter::new(config());

        let mut chunks = Vec::new();
        // Three seconds of a continuous tone: twice the limit of 1500 ms, and not one pause
        for _ in 0..6 {
            chunks.extend(segmenter.push(&tone(500)));
        }

        assert!(
            chunks.is_empty(),
            "the chunk was cut without a pause: {chunks:?}"
        );
        assert_eq!(segmenter.emitted(), 0);
        let tail = segmenter.finish().expect("the tail is the whole tone");
        assert_eq!(ms_of(&tail), 3_000, "the grown chunk lost sound");
    }

    #[test]
    fn a_drop_in_loudness_is_not_a_pause() {
        let mut segmenter = Segmenter::new(config());
        // Quieter, but well above the -45 dBFS threshold: a quiet window is not a pause, and a
        // cut there lands in the middle of a word
        let mut signal = tone(1_000);
        signal.extend(tone_at(200, 0.02));
        signal.extend(tone(600));

        assert!(
            segmenter.push(&signal).is_empty(),
            "a quiet window must not become a boundary"
        );
    }

    #[test]
    fn after_the_limit_a_pause_of_two_hundred_ms_cuts_the_chunk() {
        let mut segmenter = Segmenter::new(config_wide_pause());
        // The configured pause is 700 ms: the 220 ms pause alone would never cut
        let mut signal = tone(2_600);
        signal.extend(silence(220));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(chunks.len(), 1, "the late pause did not cut: {chunks:?}");
        let cut = ms_of(&chunks[0]);
        assert!(
            cut > 2_500,
            "the chunk is below the limit of 2500 ms: {cut} ms"
        );
        assert!(
            (2_600..=2_820).contains(&cut),
            "the cut must fall into the 220 ms pause after 2600 ms, but it came at {cut} ms"
        );
    }

    #[test]
    fn before_the_limit_a_pause_below_the_setting_does_not_cut() {
        let mut segmenter = Segmenter::new(config_wide_pause());
        // 600 ms of silence is below the configured 700, and the chunk is below the limit
        let mut signal = tone(600);
        signal.extend(silence(600));
        signal.extend(tone(600));

        assert!(
            segmenter.push(&signal).is_empty(),
            "a pause below the configured one cut the chunk before the limit"
        );
        assert_eq!(segmenter.emitted(), 0);
    }

    #[test]
    fn a_pause_that_reaches_the_setting_cuts_before_the_limit() {
        let mut segmenter = Segmenter::new(config_wide_pause());
        // 900 ms of silence reaches the configured 700, and the chunk is below the limit
        let mut signal = tone(600);
        signal.extend(silence(900));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(
            chunks.len(),
            1,
            "the configured pause did not cut: {chunks:?}"
        );
        // The middle of the pause 600…1500 ms: 1050 ms
        let cut = ms_of(&chunks[0]);
        assert!(
            (1_000..=1_100).contains(&cut),
            "the cut must be at the middle of the pause, but it came at {cut} ms"
        );
    }

    #[test]
    fn a_chunk_past_the_limit_takes_the_first_pause_the_late_rule_accepts() {
        let mut segmenter = Segmenter::new(config_wide_pause());
        // One pause of 300 ms below the configured 700, and the chunk passes the limit of 2500 ms
        let mut signal = tone(600);
        signal.extend(silence(300));
        signal.extend(tone(2_200));

        let chunks = segmenter.push(&signal);

        assert_eq!(
            chunks.len(),
            1,
            "the chunk past the limit was not cut: {chunks:?}"
        );
        // The length decides, not the place: the pause began before the limit and still cuts
        let cut = ms_of(&chunks[0]);
        assert!(
            (600..=900).contains(&cut),
            "the cut must fall into the pause at 600…900 ms, but it came at {cut} ms"
        );
    }

    #[test]
    fn a_short_recording_gives_a_single_chunk_at_the_end() {
        let mut segmenter = Segmenter::new(config());

        assert!(
            segmenter.push(&tone(300)).is_empty(),
            "it is too early to return a chunk shorter than target"
        );
        let tail = segmenter.finish().expect("there is a tail");

        assert_eq!(tail.index, 0);
        assert_eq!(ms_of(&tail), 300);
        assert_eq!(segmenter.emitted(), 1);
    }

    #[test]
    fn an_empty_recording_yields_nothing() {
        let mut segmenter = Segmenter::new(config());
        assert!(segmenter.push(&[]).is_empty());
        assert!(segmenter.finish().is_none());
        assert_eq!(segmenter.emitted(), 0);
    }

    #[test]
    fn silence_alone_never_reaches_the_model() {
        let mut segmenter = Segmenter::new(config());
        // Two seconds of silence passed both target and the limit
        assert!(
            segmenter.push(&silence(2_000)).is_empty(),
            "the silence would go to whisper and return as \"Продолжение следует\""
        );
        assert!(segmenter.finish().is_none());
    }

    #[test]
    fn neighbouring_chunks_overlap_so_a_syllable_is_not_cut_in_half() {
        let mut config = config();
        config.overlap_ms = 100;
        let mut segmenter = Segmenter::new(config);

        // Three tones with pauses between them: two boundaries, so two chunks and a tail
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));
        signal.extend(silence(400));
        signal.extend(tone(600));
        let chunks = segmenter.push(&signal);
        let tail = segmenter.finish().expect("there is a tail");

        assert_eq!(chunks.len(), 2, "two boundaries expected: {chunks:?}");
        let first_end = chunks[0].start_ms + ms_of(&chunks[0]);
        assert!(
            chunks[1].start_ms + 100 >= first_end && chunks[1].start_ms < first_end,
            "the chunks do not overlap: the first ends at {first_end}, the second starts at {}",
            chunks[1].start_ms
        );
        assert_eq!(tail.index, 2, "chunk numbering runs through");
    }

    #[test]
    fn a_pause_before_the_target_is_not_a_boundary() {
        let mut segmenter = Segmenter::new(config());
        // A pause at 200 ms is before target at 500 ms: the chunk would be too short
        let mut signal = tone(150);
        signal.extend(silence(300));
        signal.extend(tone(200));

        assert!(segmenter.push(&signal).is_empty());
        assert_eq!(segmenter.emitted(), 0);
    }

    #[test]
    fn a_stream_arriving_in_small_portions_is_cut_the_same_way() {
        let mut whole = Segmenter::new(config());
        let mut piecemeal = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));

        let at_once = whole.push(&signal);
        let mut streamed = Vec::new();
        // Portions of 100 ms: this is how the daemon takes sound from the microphone during
        // recording
        for portion in signal.chunks(RATE as usize / 10) {
            streamed.extend(piecemeal.push(portion));
        }

        assert_eq!(at_once.len(), 1);
        assert_eq!(streamed.len(), 1, "the stream in portions gave no chunk");
        // Both boundaries fell into the pause 600…1000 ms. The stream cuts earlier: once the
        // pause reaches min_pause, there is no reason to wait for its end
        for cut in [ms_of(&at_once[0]), ms_of(&streamed[0])] {
            assert!(
                (600..=1_000).contains(&cut),
                "the boundary is outside the pause: {cut} ms"
            );
        }
        assert!(
            ms_of(&streamed[0]) <= ms_of(&at_once[0]),
            "streaming cut must not wait longer than batch cut"
        );
    }

    #[test]
    fn start_ms_counts_from_the_beginning_of_the_recording() {
        let mut segmenter = Segmenter::new(config());
        let mut signal = tone(600);
        signal.extend(silence(400));
        signal.extend(tone(600));
        signal.extend(silence(400));
        signal.extend(tone(600));

        let chunks = segmenter.push(&signal);

        assert_eq!(chunks[0].start_ms, 0);
        assert!(
            chunks[1].start_ms >= 700,
            "the second chunk starts after the first: {}",
            chunks[1].start_ms
        );
    }

    #[test]
    fn defaults_match_the_documented_lengths() {
        let config = SegmenterConfig::new(16_000, DEFAULT_CHUNK_PAUSE_MS, -45.0);
        assert_eq!(config.target_chunk_secs, 3.0);
        assert_eq!(config.max_chunk_secs, 10.0);
        assert_eq!(config.overlap_ms, 150);
        assert_eq!(config.min_pause_ms, 700);
        assert_eq!(LATE_PAUSE_MS, 200);
    }

    #[test]
    fn a_zero_sample_rate_does_not_divide_by_zero() {
        let mut segmenter = Segmenter::new(SegmenterConfig::new(0, 700, -45.0));
        assert!(segmenter.push(&[0.5; 100]).is_empty());
        let tail = segmenter.finish().expect("the tail is returned as it is");
        assert_eq!(tail.start_ms, 0);
    }
}
