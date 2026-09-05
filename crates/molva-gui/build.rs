// SPDX-License-Identifier: MIT
//! Кодогенерация Tauri: разрешения, ресурсы, метаданные пакета.
//!
//! Плюс одна поправка для Linux. Трей грузит `libayatana-appindicator3` через `dlopen`
//! в момент создания значка, а в devShell от Nix эта библиотека лежит в store и не видна
//! ни в `ld.so.cache`, ни в `LD_LIBRARY_PATH`. Спрашиваем её каталог у pkg-config на этапе
//! сборки и прописываем в RUNPATH исполняемого файла: `dlopen` из самого бинаря его читает.

fn main() {
    #[cfg(target_os = "linux")]
    add_appindicator_runpath();
    tauri_build::build();
}

#[cfg(target_os = "linux")]
fn add_appindicator_runpath() {
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    // Разные дистрибутивы называют пакет по-разному; берём первый найденный.
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
    // Библиотеки нет — значок в трее просто не появится, приложение это переживёт.
    println!("cargo:warning=libayatana-appindicator не найден: значок в трее будет недоступен");
}
