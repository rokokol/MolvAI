// SPDX-License-Identifier: MIT
//! Inter-process communication of the daemon with the CLI and the GUI

pub mod protocol;

pub use protocol::{
    Command, DaemonState, ErrorCode, Event, IpcError, Request, Response, PROTOCOL_VERSION,
};
