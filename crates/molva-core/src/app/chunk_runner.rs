// SPDX-License-Identifier: MIT
//! Chunks of one file, recognized side by side
//!
//! A file cut at its pauses holds hundreds of chunks, and a recognition over the network answers
//! each of them in a few hundred milliseconds: one after another, a ten-minute file spends its
//! time waiting rather than working. An engine that reports a replica has nothing local to share,
//! so several chunks go out at once, and the texts come back in the order of the chunks whatever
//! order the answers arrived in.
//!
//! A model that runs in this process reports no replica: a second copy would compete with the
//! first for the same cores, and the pipeline keeps such an engine on one chunk at a time

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::domain::stt::{SttEngine, SttError, SttOptions, Transcript};

/// How many requests one file may have in flight
///
/// Four covers the latency of a provider without turning a phrase into a burst that a rate limit
/// answers with 429, and the retry of the engine then works through such an answer on its own
pub const REMOTE_WORKERS: usize = 4;

/// What one chunk came to: its transcript, or the reason it has none
pub type ChunkResult = Result<Transcript, SttError>;

/// Recognize every chunk with up to `workers` engines working at the same time
///
/// `replica` is asked for the engines beside the first one; `cancelled` is read before each
/// chunk, so a job the client gave up on stops sending. A chunk that failed keeps its place as
/// an error, and the caller decides whether the whole file fails or the text simply loses a part
pub fn recognize_chunks(
    engine: &mut dyn SttEngine,
    chunks: &[crate::domain::audio::PcmAudio],
    options: &SttOptions,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<ChunkResult>, SttError> {
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let mut engines: Vec<Box<dyn SttEngine>> = Vec::new();
    for _ in 1..chunks.len().min(REMOTE_WORKERS.max(1)) {
        // An engine that cannot be copied makes the whole run sequential: mixing copies of an
        // unknown reliability is worse than taking longer
        let Some(replica) = engine.replica() else {
            engines.clear();
            break;
        };
        engines.push(replica);
    }

    if engines.is_empty() {
        let mut results = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            if cancelled() {
                return Err(SttError::Cancelled);
            }
            results.push(transcribe_one(engine, chunk, options));
        }
        return Ok(results);
    }

    let workers = engines.len() + 1;
    let results: Mutex<Vec<Option<ChunkResult>>> =
        Mutex::new((0..chunks.len()).map(|_| None).collect());
    let cursor = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for replica in engines.drain(..) {
            handles.push(scope.spawn(|| {
                let mut replica = replica;
                loop {
                    if cancelled() {
                        return;
                    }
                    let index = cursor.fetch_add(1, Ordering::SeqCst);
                    let Some(chunk) = chunks.get(index) else {
                        return;
                    };
                    let result = transcribe_one(replica.as_mut(), chunk, options);
                    if let Ok(mut slots) = results.lock() {
                        slots[index] = Some(result);
                    }
                }
            }));
        }
        // The first engine works beside the copies, and it is the one the caller keeps
        let first = &mut *engine;
        loop {
            if cancelled() {
                break;
            }
            let index = cursor.fetch_add(1, Ordering::SeqCst);
            let Some(chunk) = chunks.get(index) else {
                break;
            };
            let result = transcribe_one(first, chunk, options);
            if let Ok(mut slots) = results.lock() {
                slots[index] = Some(result);
            }
        }
        for handle in handles {
            let _ = handle.join();
        }
    });

    if cancelled() {
        return Err(SttError::Cancelled);
    }
    let slots = results.into_inner().unwrap_or_default();
    let mut ordered = Vec::with_capacity(slots.len());
    for slot in slots {
        match slot {
            Some(result) => ordered.push(result),
            // A chunk that never ran because of a cancel keeps the shape of the answer: the
            // caller sees a cancelled result rather than a shorter list
            None => ordered.push(Err(SttError::Cancelled)),
        }
    }
    Ok(ordered)
}

fn transcribe_one(
    engine: &mut dyn SttEngine,
    chunk: &crate::domain::audio::PcmAudio,
    options: &SttOptions,
) -> ChunkResult {
    crate::infra::stt::transcribe_with_language_policy(engine, chunk, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::audio::PcmAudio;
    use crate::domain::stt::SttEngine;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// An engine that can be copied: it counts how many of its copies work at the same time
    #[derive(Debug)]
    struct Slow {
        live: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        delay: Duration,
        /// Text of every chunk carries the number of the call, so the order is visible
        counter: Arc<AtomicUsize>,
    }

    impl Slow {
        fn new(delay: Duration) -> Self {
            Self {
                live: Arc::new(AtomicUsize::new(0)),
                peak: Arc::new(AtomicUsize::new(0)),
                delay,
                counter: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    impl SttEngine for Slow {
        #[allow(clippy::unnecessary_literal_bound)]
        fn id(&self) -> &str {
            "slow"
        }
        #[allow(clippy::unnecessary_literal_bound)]
        fn model_name(&self) -> &str {
            "slow"
        }
        fn transcribe(
            &mut self,
            _audio: &PcmAudio,
            _options: &SttOptions,
        ) -> Result<Transcript, SttError> {
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.live.fetch_sub(1, Ordering::SeqCst);
            let call = self.counter.fetch_add(1, Ordering::SeqCst);
            Ok(Transcript::text_only(format!("chunk {call}")))
        }
        fn unload(&mut self) {}
        fn replica(&self) -> Option<Box<dyn SttEngine>> {
            Some(Box::new(Self {
                live: Arc::clone(&self.live),
                peak: Arc::clone(&self.peak),
                delay: self.delay,
                counter: Arc::clone(&self.counter),
            }))
        }
    }

    fn chunks(count: usize) -> Vec<PcmAudio> {
        (0..count)
            .map(|_| PcmAudio::new(vec![0.1; 1600], 16_000))
            .collect()
    }

    fn no_cancel() -> bool {
        false
    }

    #[test]
    fn the_chunks_of_a_cloud_file_run_side_by_side() {
        let mut engine = Slow::new(Duration::from_millis(120));
        let started = Instant::now();
        let results = recognize_chunks(&mut engine, &chunks(8), &SttOptions::default(), &no_cancel)
            .expect("no chunk fails");
        let elapsed = started.elapsed();

        assert_eq!(results.len(), 8);
        assert!(engine.peak() > 1, "the requests went out one at a time");
        // Eight chunks of 120 ms in a row would take most of a second
        assert!(
            elapsed < Duration::from_millis(700),
            "the run took {elapsed:?}"
        );
        for result in results {
            assert!(result.expect("a transcript").text.starts_with("chunk"));
        }
    }

    #[test]
    fn the_texts_come_back_in_the_order_of_the_chunks() {
        // The delays differ, so the answers arrive out of order: the list must not
        let mut engine = Slow::new(Duration::ZERO);
        let results =
            recognize_chunks(&mut engine, &chunks(12), &SttOptions::default(), &no_cancel)
                .expect("no chunk fails");
        let texts: Vec<String> = results
            .into_iter()
            .map(|result| result.expect("a transcript").text)
            .collect();
        assert_eq!(texts.len(), 12);
        let mut numbers: Vec<usize> = texts
            .iter()
            .map(|text| {
                text.trim_start_matches("chunk ")
                    .parse()
                    .expect("the call number")
            })
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(numbers.len(), 12, "every chunk is recognized once");
    }

    #[test]
    fn an_engine_without_a_replica_runs_one_chunk_at_a_time() {
        #[derive(Debug)]
        struct Lonely;
        impl SttEngine for Lonely {
            #[allow(clippy::unnecessary_literal_bound)]
            fn id(&self) -> &str {
                "lonely"
            }
            #[allow(clippy::unnecessary_literal_bound)]
            fn model_name(&self) -> &str {
                "lonely"
            }
            fn transcribe(
                &mut self,
                _audio: &PcmAudio,
                _options: &SttOptions,
            ) -> Result<Transcript, SttError> {
                Ok(Transcript::text_only("alone"))
            }
            fn unload(&mut self) {}
        }

        let results = recognize_chunks(&mut Lonely, &chunks(3), &SttOptions::default(), &no_cancel)
            .expect("no chunk fails");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn a_cancelled_job_stops_sending_chunks() {
        let engine = Slow::new(Duration::from_millis(40));
        let cancelled = AtomicBool::new(false);
        let flag = &cancelled;
        let check = || flag.load(Ordering::SeqCst);
        let chunks = chunks(40);
        let outcome = std::thread::scope(|scope| {
            let cancelling = scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                flag.store(true, Ordering::SeqCst);
            });
            let outcome =
                recognize_chunks(&mut { engine }, &chunks, &SttOptions::default(), &check);
            let _ = cancelling.join();
            outcome
        });
        assert!(matches!(outcome, Err(SttError::Cancelled)), "{outcome:?}");
    }

    #[test]
    fn an_empty_file_asks_the_engine_nothing() {
        let mut engine = Slow::new(Duration::ZERO);
        let results = recognize_chunks(&mut engine, &[], &SttOptions::default(), &no_cancel)
            .expect("an empty list is not an error");
        assert!(results.is_empty());
        assert_eq!(engine.peak(), 0);
    }
}
