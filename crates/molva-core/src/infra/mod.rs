// SPDX-License-Identifier: MIT
//! Infrastructure: real implementations of the `domain` contracts on top of the OS, the network and external libraries
//!
//! One declaration per line: several tracks share this file, so there are fewer conflicts

pub mod audio;
pub mod hotkeys;
pub mod inject;
pub mod ipc;
pub mod llm;
pub mod notify;
pub mod platform;
pub mod sound;
pub mod stt;
