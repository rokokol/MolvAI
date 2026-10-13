# Changelog

This file follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

The entries up to 0.1.9 were written after the fact, from the release tags and the git history

## Unreleased

### Added

- The GUI tray menu, its tooltip, system notices and setting errors follow the interface language, Russian or English
- A daemon error in the GUI starts with a title in the interface language, and the daemon's English text follows it

### Changed

- The daemon and the CLI are English only: messages, errors, help text, table headers and "molva doctor" output
- The built-in style names in the CLI are English: Verbatim, Cleanup, Messenger, Email, Code and Formal
- A GUI string with no translation falls back to English instead of Russian
- The Hyprland snippet in the GUI settings has English comments
- The documentation is in English

## [0.1.9] - 2026-10-13

### Added

- A silent microphone now shows a system notice and a message in the window, not only a log line
- "molva doctor" names the cause and the next step when the LLM HTTP client cannot start

### Changed

- Errors from the LLM HTTP client now include their cause
- "nix build .#molva" now passes, tests included
- Update symphonia to 0.6, chacha20poly1305 to 0.11 and sha2 to 0.11, with no change in behaviour
- Update TypeScript to 7, Vite to 8 and React to 19.3 in the GUI, with no change in behaviour

### Fixed

- The macOS build asks for microphone access, so it no longer records silence. This fix is not yet tested on real hardware

### Security

- Update quinn-proto to close three denial-of-service advisories: RUSTSEC-2026-0338, RUSTSEC-2026-0340 and RUSTSEC-2026-0343

## [0.1.8] - 2026-09-30

### Added

- Linux and Windows builds run whisper on the graphics card through Vulkan
- The GUI language can follow the system, with the new default "auto" and the new choice "System language"

### Changed

- The builds fall back to the processor when no Vulkan device exists
- The GUI language default now comes from the system and is English for every non-Russian locale
- A configuration with "ru" or "en" already set keeps its language

## [0.1.7] - 2026-09-29

### Added

- The setting "stt.models_mirror" and a GUI field let you download model weights from a mirror when Hugging Face is not reachable
- The GUI panel shows why the daemon that it started has failed

### Changed

- Linux packages now build on ubuntu-22.04 and run on glibc 2.35
- The macOS app now has an ad-hoc signature

### Fixed

- Paste on Windows and macOS works with the Russian keyboard layout

## [0.1.6] - 2026-09-29

### Added

- The GUI panel shows the model download before the daemon opens its socket

### Fixed

- The AppImage now starts when the user does not own the files, so firejail "--appimage" and the appimage.github.io catalog test accept it
- "install.sh" now rejects an option that has no value with a clear message

### Security

- Update rustls to 0.23.45 to close RUSTSEC-2026-0285

## [0.1.5] - 2026-09-06

### Fixed

- The Windows CUDA build of the CLI passes "-allow-unsupported-compiler" to nvcc

## [0.1.4] - 2026-09-06

### Fixed

- The Windows CUDA build of the CLI runs from PowerShell

## [0.1.3] - 2026-09-06

### Fixed

- GUI packages now take their version from "Cargo.toml", so they no longer carry the name "MolvAI_0.1.0" in releases 0.1.1 and 0.1.2
- The Windows CUDA build of the CLI now builds through Ninja

## [0.1.2] - 2026-09-06

### Added

- The daemon downloads missing model weights when it starts
- The release includes a separate Windows archive of the CLI with CUDA support

## [0.1.1] - 2026-09-06

### Added

- The GUI packages include the "molva" CLI

### Fixed

- Hyprland key combinations use key codes, so they work with any keyboard layout

## [0.1.0] - 2026-09-06

First release, made for the V0ICE hackathon

### Added

- Default hotkeys F9 and Ctrl+Alt
- Voice editing of the selected text: command mode takes the selection through the Ctrl+C of the compositor
- Encryption of the journal texts, set in the configuration, and a reload of the daemon settings while it runs
- The command "molva config reload" asks the daemon to read the settings again
- Secret storage in the system keyring with a file fallback, for the provider key and the journal key
- The commands "molva secret set", "molva secret delete" and "molva secret status", and a keyring check in "molva doctor"
- The daemon records the next phrase while it processes the last one

### Fixed

- A key press during the processing of an earlier phrase no longer gets lost
- A short tap on the key no longer turns into a hold when the microphone is slow
- Command mode takes only a fresh selection, not an old clipboard
- Command mode returns only the result, without headings
- The style choice in the panel now reaches the daemon and stays in the settings
- The "cleanup" style keeps addresses and endearments

## [0.1.0-alpha.1] - 2026-09-06

### Changed

- The release build of the GUI runs "npm ci" first, so the front end builds

## [0.1.0-alpha] - 2026-09-06

Nightly snapshot of the hackathon: all tracks merged and CI green on three operating systems

### Added

- Voice dictation: microphone capture, whisper.cpp speech recognition, post-processing rules, an optional LLM with styles, and insertion of the text into the active window
- A background daemon with a command-line interface: "molva daemon", "record", "status", "setup", "doctor", "transcribe", "models", "bench", "history", "stats", "styles", "dictionary" and "config"
- A Tauri desktop app with a tray icon, a panel, a history, statistics, a file page and settings
- A term dictionary with exact and fuzzy match
- A file journal of phrases in JSONL

[Unreleased]: https://github.com/rokokol/MolvAI/compare/v0.1.9...HEAD
[0.1.9]: https://github.com/rokokol/MolvAI/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/rokokol/MolvAI/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/rokokol/MolvAI/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/rokokol/MolvAI/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/rokokol/MolvAI/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/rokokol/MolvAI/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/rokokol/MolvAI/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/rokokol/MolvAI/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/rokokol/MolvAI/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/rokokol/MolvAI/compare/v0.1.0-alpha.1...v0.1.0
[0.1.0-alpha.1]: https://github.com/rokokol/MolvAI/compare/v0.1.0-alpha...v0.1.0-alpha.1
[0.1.0-alpha]: https://github.com/rokokol/MolvAI/releases/tag/v0.1.0-alpha
