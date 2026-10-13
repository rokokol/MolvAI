// SPDX-License-Identifier: MIT
//! The clock as a dependency: daily statistics and timeouts are tested without waiting

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

pub trait Clock: std::fmt::Debug + Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
    fn instant(&self) -> Instant;
    /// Wait. This is the only way to pause in product code: a direct `std::thread::sleep`
    /// makes a test of the pause either slow or unprovable
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_utc(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }
}
