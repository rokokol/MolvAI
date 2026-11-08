// SPDX-License-Identifier: MIT
//! Inter-process communication of the daemon with the CLI and the GUI

pub mod protocol;

pub use protocol::{
    Command, DaemonState, ErrorCode, Event, FileFailure, FileReport, FileTranscriptionRequest,
    IpcError, Request, Response, WorkerJob, PROTOCOL_VERSION,
};
