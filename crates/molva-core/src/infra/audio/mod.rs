// SPDX-License-Identifier: MIT
//! Захват с микрофона через cpal.

pub mod cpal_source;
pub mod level;

pub use cpal_source::{list_input_devices, CpalSource};
pub use level::ZeroLevelWatch;
