// SPDX-License-Identifier: MIT
//! MolvAI core
//!
//! Layers:
//! - `domain` — types, traits and pure functions with no dependency on frameworks or hardware
//! - `app` — pipeline, daemon, config, journal (they appear as the tracks are implemented)
//! - `infra` — real implementations of the traits: cpal, whisper.cpp, HTTP, paste, evdev
//! - `ipc` — the protocol between the daemon, the CLI and the GUI
//!
//! Tests reach the hardware through the trait fakes in `domain::fakes`, not through feature flags

// In tests a panic is the way to report a failure, not an unhandled error. An exact comparison
// of floating-point numbers is valid (the test compares a computed value with a constant that
// the test itself set). Printing measurements to stdout is the report of a benchmark test
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::float_cmp,
        clippy::print_stdout
    )
)]

pub mod app;
pub mod config;
pub mod domain;
pub mod infra;
pub mod ipc;

pub use config::Config;
