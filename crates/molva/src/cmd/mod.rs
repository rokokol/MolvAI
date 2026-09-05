// SPDX-License-Identifier: MIT
//! Подкоманды CLI. Каждая — отдельный файл, чтобы дорожки не дрались за один модуль.

pub mod daemon;
pub mod doctor;
pub mod record;
pub mod setup;
pub mod status;
pub mod test_inject;
