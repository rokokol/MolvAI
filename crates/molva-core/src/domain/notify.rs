// SPDX-License-Identifier: MIT
//! Notifications to the user: each error message names the next step

pub trait Notifier: std::fmt::Debug + Send + Sync {
    fn notify(&self, title: &str, body: &str);
}
