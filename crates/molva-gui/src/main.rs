// SPDX-License-Identifier: MIT
//! Запуск GUI MolvAI.

// В релизной сборке под Windows не открывать консольное окно рядом с приложением.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // NVIDIA + Wayland: без этой переменной webkit рисует пустое окно. Ставить её нужно
    // до создания webview, поэтому это единственное место в проекте, где мы пишем
    // в окружение процесса, — и только если пользователь не задал значение сам.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    molva_gui::run();
}
