// SPDX-License-Identifier: MIT
//! Транспорт IPC поверх локального сокета.

pub mod transport;

pub use transport::{
    ping, socket_path, Client, Events, IpcClientError, IpcServerError, RequestHandler, Server,
    Stopper,
};
