// SPDX-License-Identifier: MIT
//! Межпроцессное взаимодействие демона с CLI и GUI.

pub mod protocol;

pub use protocol::{
    Command, DaemonState, ErrorCode, Event, IpcError, Request, Response, PROTOCOL_VERSION,
};
