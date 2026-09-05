// SPDX-License-Identifier: MIT
//! Захват с микрофона через cpal.

pub mod cpal_source;

pub use cpal_source::{list_input_devices, CpalSource};
