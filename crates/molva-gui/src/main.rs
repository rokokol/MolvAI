// SPDX-License-Identifier: MIT
//! Entry point of the MolvAI GUI

// In a release build on Windows, do not open a console window beside the application
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // NVIDIA + Wayland: without this variable webkit draws an empty window. We must set it
    // before the webview is created. So this is the only place in the project where we
    // write to the process environment, and only if the user did not set the value
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    molva_gui::run();
}
