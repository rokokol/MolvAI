// SPDX-License-Identifier: MIT
//! Audio infrastructure: microphone capture through cpal and file reading
//!
//! One declaration per line: several tracks share this file, so there are fewer conflicts

pub mod cpal_source;
pub mod decode;
pub mod level;

pub use cpal_source::{list_input_devices, CpalSource};
pub use level::ZeroLevelWatch;
