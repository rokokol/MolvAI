// SPDX-License-Identifier: MIT
//! Tauri code generation: permissions, resources, package metadata
//!
//! It also applies one fix for Linux. The tray loads `libayatana-appindicator3` with `dlopen`
//! when it creates the icon. In the Nix devShell this library sits in the store. Neither
//! `ld.so.cache` nor `LD_LIBRARY_PATH` shows it. At build time we ask pkg-config for its
//! directory and write it to the RUNPATH of the executable: `dlopen` called from the binary
//! itself reads that RUNPATH

fn main() {
    #[cfg(target_os = "linux")]
    add_appindicator_runpath();
    tauri_build::build();
}

#[cfg(target_os = "linux")]
fn add_appindicator_runpath() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    // Distributions name the package in different ways; take the first one found
    for package in ["ayatana-appindicator3-0.1", "appindicator3-0.1"] {
        let output = std::process::Command::new("pkg-config")
            .args(["--variable=libdir", package])
            .output();
        let Ok(output) = output else { continue };
        if !output.status.success() {
            continue;
        }
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if dir.is_empty() {
            continue;
        }
        println!("cargo:rustc-link-arg=-Wl,-rpath,{dir}");
        return;
    }
    // Without the library the tray icon does not appear, and the application survives that
    println!("cargo:warning=libayatana-appindicator not found: the tray icon will be unavailable");
}
