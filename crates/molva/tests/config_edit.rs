// SPDX-License-Identifier: MIT
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[cfg(target_os = "linux")]
#[test]
fn editor_receives_a_private_file_and_saves_the_secret() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[llm]\napi_key_plain = \"secret-for-test\"\n").unwrap();
    let editor = dir.path().join("editor");
    std::fs::write(&editor, "#!/bin/sh\ntest \"$(stat -c %a \"$1\")\" = 600\n").unwrap();
    std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_molva"))
        .args(["--config", config.to_str().unwrap(), "config", "edit"])
        .env("TMPDIR", &tmp)
        .env("VISUAL", &editor)
        .env_remove("MOLVA_RUNTIME_CONFIG")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(tmp).unwrap().count(), 0);
    assert!(std::fs::read_to_string(config)
        .unwrap()
        .contains("secret-for-test"));
}

#[test]
fn editor_failures_remove_the_secret_copy() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir(&tmp).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[llm]\napi_key_plain = \"secret-for-test\"\n").unwrap();
    let editor = dir.path().join("editor");
    for script in [
        None,
        Some("#!/bin/sh\nexit 1\n"),
        Some("#!/bin/sh\nprintf 'invalid = = toml' > \"$1\"\n"),
    ] {
        if let Some(script) = script {
            std::fs::write(&editor, script).unwrap();
            std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_molva"))
            .args(["--config", config.to_str().unwrap(), "config", "edit"])
            .env("TMPDIR", &tmp)
            .env("VISUAL", &editor)
            .env_remove("MOLVA_RUNTIME_CONFIG")
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("secret-for-test"));
    }
}
