// SPDX-License-Identifier: MIT
//! Entry point of the MolvAI GUI

// In a release build on Windows, do not open a console window beside the application
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // NVIDIA + Wayland: without this variable webkit draws an empty window. It must be set
    // before the webview is created. So this is the only place in the project that writes to
    // the process environment, and only if the user did not set the value
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // Native Wayland through GTK gives this WebView a negative device scale and a viewport
        // in the millions of pixels on Hyprland. XWayland gives the same page correct metrics.
        // The reason and the removal check are in WORKAROUNDS.md; the variable must be set
        // before GTK starts
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
            && std::env::var_os("GDK_BACKEND").is_none()
        {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }
    molva_gui::run();
}
