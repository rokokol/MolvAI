// SPDX-License-Identifier: MIT
//! Scheduling of model calls: a local model is serial, a cloud provider is not
//!
//! One policy for every caller, and the caller is any stage that needs text from a model: the
//! post-processing of a phrase and voice editing. The policy is one object, so no stage can
//! decide on its own whether it may wait for the network beside another call.
//!
//! - A local provider (Ollama, LM Studio, any endpoint on the loopback interface) loads one
//!   request at a time. A second call waits on one mutex, and the model is never asked for two
//!   answers at once: it would answer both slower and worse.
//! - A cloud provider (OpenAI, OpenRouter, Groq, a custom remote URL) answers requests
//!   independently. They run at the same time, up to [`CLOUD_SLOTS`]: enough for independent
//!   phrases, and few enough that a rate limit is not the first thing the user learns about. A
//!   caller beyond the bound waits for a free slot, and every call is itself bounded by
//!   `llm.timeout_secs`, so that wait is finite.
//!
//! Privacy is decided before this object: `build_llm` returns `None` unless the model is enabled
//! and `privacy.send_to_llm` allows a call, so a request that arrives here may leave the machine.
//! A failure is returned unchanged: the fallback text of the phrase stays where it was.
//!
//! The seam for a still asynchronous delivery is this type as well. Today every caller waits for
//! its own answer, because the text must be pasted before the next phrase; the recognition of the
//! next phrase is therefore queued behind a cloud call of the previous one. A stage that wants to
//! recognize ahead has to hand its request to this policy and keep the handle, and the paste
//! order then belongs to that stage. Nothing below this module changes for that: the policy,
//! the bound and the local serialization stay as they are.

use std::sync::{Arc, Condvar, Mutex, PoisonError};

use crate::domain::llm::{ChatRequest, ChatResponse, LlmClient, LlmError};

/// How many calls to a cloud provider may run at the same time
pub const CLOUD_SLOTS: usize = 4;

/// A local model: one request at a time, and the caller waits for it on its own thread
#[derive(Debug)]
pub struct LlmExecutor {
    client: Arc<dyn LlmClient>,
    /// The local model takes one request at a time
    local: Mutex<()>,
    /// Free slots of a cloud provider
    cloud: CloudSlots,
}

impl LlmExecutor {
    pub fn new(client: Arc<dyn LlmClient>) -> Self {
        Self::with_slots(client, CLOUD_SLOTS)
    }

    /// The same, with a number of cloud slots of the caller's choice: a test needs a bound it
    /// can observe, and a deployment may know better than the default
    pub fn with_slots(client: Arc<dyn LlmClient>, slots: usize) -> Self {
        Self {
            client,
            local: Mutex::new(()),
            cloud: CloudSlots::new(slots),
        }
    }

    /// Wrap a client that may be absent: the single place that builds the executor
    pub fn wrap(client: Option<Arc<dyn LlmClient>>) -> Option<Arc<LlmExecutor>> {
        client.map(|client| Arc::new(Self::new(client)))
    }

    /// The client under the policy, for a caller that needs its own identifiers
    pub fn client(&self) -> &Arc<dyn LlmClient> {
        &self.client
    }

    /// Whether the model runs on this machine
    pub fn is_local(&self) -> bool {
        self.client.is_local()
    }
}

impl LlmClient for LlmExecutor {
    fn id(&self) -> &str {
        self.client.id()
    }

    fn is_local(&self) -> bool {
        self.client.is_local()
    }

    fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
        if self.client.is_local() {
            // The guard lives until the answer: two phrases never reach a local model together
            let _serial = self.local.lock().unwrap_or_else(PoisonError::into_inner);
            return self.client.complete(request);
        }
        let _slot = self.cloud.take();
        self.client.complete(request)
    }
}

/// Free slots of a cloud provider
#[derive(Debug)]
struct CloudSlots {
    free: Mutex<usize>,
    freed: Condvar,
}

impl CloudSlots {
    fn new(slots: usize) -> Self {
        Self {
            // A bound of zero would never let a call through: one slot is the smallest bound
            free: Mutex::new(slots.max(1)),
            freed: Condvar::new(),
        }
    }

    /// Take a slot, waiting while every one of them is busy
    fn take(&self) -> CloudSlot<'_> {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        while *free == 0 {
            free = self
                .freed
                .wait(free)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *free -= 1;
        CloudSlot(self)
    }

    fn give_back(&self) {
        let mut free = self.free.lock().unwrap_or_else(PoisonError::into_inner);
        *free += 1;
        self.freed.notify_one();
    }
}

/// A taken slot: the answer releases it, and so does a panic on the way
struct CloudSlot<'a>(&'a CloudSlots);

impl Drop for CloudSlot<'_> {
    fn drop(&mut self) {
        self.0.give_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A client that holds every call for a while and reports how many ran together
    #[derive(Debug)]
    struct SlowClient {
        local: bool,
        active: AtomicUsize,
        peak: AtomicUsize,
        calls: AtomicUsize,
        hold: Duration,
    }

    impl SlowClient {
        fn new(local: bool, hold: Duration) -> Arc<Self> {
            Arc::new(Self {
                local,
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                calls: AtomicUsize::new(0),
                hold,
            })
        }

        fn peak(&self) -> usize {
            self.peak.load(Ordering::SeqCst)
        }
    }

    impl LlmClient for SlowClient {
        fn id(&self) -> &'static str {
            if self.local {
                "ollama"
            } else {
                "openrouter"
            }
        }

        fn is_local(&self) -> bool {
            self.local
        }

        fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, LlmError> {
            let running = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(running, Ordering::SeqCst);
            std::thread::sleep(self.hold);
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ChatResponse {
                text: request.user.clone(),
                prompt_tokens: None,
                completion_tokens: None,
                cost_usd: None,
                note: None,
            })
        }
    }

    fn request(text: &str) -> ChatRequest {
        ChatRequest {
            model: "m".into(),
            system: "s".into(),
            user: text.into(),
            temperature: 0.0,
            max_tokens: 16,
        }
    }

    /// Run `count` calls at the same time and return the peak reported by the client
    fn run_together(executor: &Arc<LlmExecutor>, count: usize) {
        let threads: Vec<_> = (0..count)
            .map(|index| {
                let executor = executor.clone();
                std::thread::spawn(move || {
                    executor
                        .complete(&request(&format!("текст {index}")))
                        .expect("the client answers")
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("the call thread finishes");
        }
    }

    #[test]
    fn cloud_calls_run_at_the_same_time() {
        let client = SlowClient::new(false, Duration::from_millis(200));
        let executor = Arc::new(LlmExecutor::new(client.clone()));
        run_together(&executor, 3);
        assert!(
            client.peak() >= 2,
            "the cloud calls ran one after another (peak {})",
            client.peak()
        );
    }

    #[test]
    fn a_local_model_is_never_asked_two_questions_at_once() {
        let client = SlowClient::new(true, Duration::from_millis(80));
        let executor = Arc::new(LlmExecutor::new(client.clone()));
        run_together(&executor, 3);
        assert_eq!(
            client.peak(),
            1,
            "a local model got more than one request at a time"
        );
        assert_eq!(client.calls.load(Ordering::SeqCst), 3, "a call was lost");
    }

    #[test]
    fn the_cloud_bound_holds_when_more_calls_arrive_than_slots_exist() {
        let client = SlowClient::new(false, Duration::from_millis(60));
        let executor = Arc::new(LlmExecutor::with_slots(client.clone(), 1));
        run_together(&executor, 3);
        assert_eq!(client.peak(), 1, "the bound of one slot was exceeded");
        assert_eq!(client.calls.load(Ordering::SeqCst), 3, "a call was lost");
    }

    #[test]
    fn a_bound_of_zero_still_lets_one_call_through() {
        let client = SlowClient::new(false, Duration::ZERO);
        let executor = LlmExecutor::with_slots(client.clone(), 0);
        executor.complete(&request("текст")).expect("an answer");
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_identifiers_of_the_client_are_kept() {
        let cloud = LlmExecutor::new(SlowClient::new(false, Duration::ZERO));
        assert_eq!(cloud.id(), "openrouter");
        assert!(!cloud.is_local());
        let local = LlmExecutor::new(SlowClient::new(true, Duration::ZERO));
        assert_eq!(local.id(), "ollama");
        assert!(local.is_local());
        assert!(LlmExecutor::wrap(None).is_none());
    }

    #[test]
    fn a_failure_comes_back_unchanged_through_the_policy() {
        let client = crate::domain::fakes::FakeLlm::failing(LlmError::Auth);
        let executor = LlmExecutor::new(Arc::new(client));
        assert_eq!(executor.complete(&request("текст")), Err(LlmError::Auth));
    }
}
