// SPDX-License-Identifier: MIT
//! Часы как зависимость: статистика по дням и таймауты тестируются без ожидания.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

pub trait Clock: std::fmt::Debug + Send + Sync {
    fn now_utc(&self) -> DateTime<Utc>;
    fn instant(&self) -> Instant;
    /// Подождать. Единственный способ сделать паузу в коде продукта: `std::thread::sleep`
    /// напрямую делает тест на паузу либо медленным, либо недоказуемым.
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
