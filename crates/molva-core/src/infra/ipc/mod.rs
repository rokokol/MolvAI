// SPDX-License-Identifier: MIT
//! IPC transport over a local socket

pub mod transport;

pub use transport::{
    ping, socket_path, Client, Events, IpcClientError, IpcServerError, RequestHandler, Server,
    Stopper,
};
