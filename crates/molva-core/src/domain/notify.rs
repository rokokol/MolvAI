// SPDX-License-Identifier: MIT
//! Уведомления пользователю: каждое сообщение об ошибке называет следующий шаг.

pub trait Notifier: Send + Sync {
    fn notify(&self, title: &str, body: &str);
}
