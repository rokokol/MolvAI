# Changelog

This file follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

The entries up to 0.1.9 were written after the fact, from the release tags and the git history

## Unreleased

## [0.3.1] - 2026-11-10

### Added

- The Files tab plays the file it transcribed: a video plays in a preview beside its transcript, and an audio file gets a waveform of its loudness with the played part filled in

- The transcript of a file scrolls to the segment that plays, and a press on a segment moves the player there

### Changed

- One result is laid out as a small player and a scrolling transcript beside it, instead of a player the height of the window

- A file cut at its pauses gets one segment per chunk, with the pauses as its bounds, instead of the inner segments of the model that came out shorter than a second

- The rule that puts a capital letter at the start of a sentence is called "Capital letters" in the settings, which is what it does

- The MCP switch reads "Enable MCP server", and the window keeps working with the server switched off

- The release notes are the changelog section of the version, with its links pinned to the tag, instead of a generated summary

### Fixed

- A file transcribed without "Cut at pauses" came out as fragments under a second: GigaAM reports one token every forty milliseconds, and the grouping cut them at a pause of four hundred, which happens inside a phrase. The cut now needs seven hundred milliseconds of silence and a segment of at least 1.2 seconds, and shorter pieces join their neighbour

- The "Cut at pauses" switch of the Files tab changed nothing: the queue value was memoized without it, so the tab kept the state of the first render

- Playing a file jumped back to the moment of the last press on every frame, because the seek effect depended on a callback that changed with each render

- Drawing the loudness of a long file froze the window for minutes: the decode held every sample in memory and ran on the thread that answers the window

## [0.3.0] - 2026-11-10

### Added

- Runtime settings overrides: a managed Nix config stays editable in the GUI and through `molva config set`; changes go to `~/.config/molva/runtime.toml` and last until a rebuild changes the declarative config

- `molva config path` prints the runtime file too, `molva config edit` edits a plain copy, and the Settings page shows where the changes land

- The Pause hotkeys tray switch stops the binds that start a phrase; the GUI and MCP commands, the key release and cancel stay live

- Every style carries its own word minimum for the model. Built-in styles start at three words, custom styles have an editable field, and `molva styles list` and `molva styles show` print it

- The model and recognition test buttons run their checks in the daemon, which already holds the API key and the engine factory; the window only translates the answer

### Changed

- The built-in styles are "As spoken", Cleanup and Formal; "As spoken" never calls the model

- Built-in style prompts and the command-mode prompt are shorter and send fewer tokens

- `rules.llm_min_words` is gone: the threshold belongs to a style, so a short phrase is decided by the style that will process it

- `programs.molvai.showNotice` and `gui.show_managed_notice` are gone: settings are editable at runtime, so the managed banner has nothing left to say

- `docs/STATUS.md` is gone; release history lives in the changelog and the site status page keeps only the current summary

### Fixed

- A failed phrase opens one system notification with the reason, instead of two; the notification icon uses a stable content-addressed path

- Opening a spectrogram of a long stereo file no longer doubles the decoded audio in memory

- An entry that finished with a model or paste error shows the reason on the Dashboard

- The LLM and recognition test buttons sit next to their status inside Settings

- Retyping the identifier of a custom style moves `style.default` and the per-window rules to the new name instead of failing the save

- A test of a cloud model no longer fails with a missing key: the check reads the key in the daemon, where the Nix service provides it

- Selecting a style in the GUI applies it on a managed config

- An HTTP 403 from a model provider is reported as unavailable with the provider's reason, not as a rejected API key

## [0.2.11] - 2026-11-09

### Fixed

- The CUDA release job installs the Wayland, XKB and ALSA headers that the shared file pipeline now needs

## [0.2.10] - 2026-11-09

### Fixed

- Clippy accepts the decoded audio passed into the shared file pipeline

## [0.2.9] - 2026-11-09

### Added

- `molva styles next` and the `style_next` hotkey switch the post-processing style in a circle

- A hidden window shows the new style name in a system notification

- A `chunked` option cuts a file at pauses before recognition, in the CLI, the Files tab and the MCP `transcribe` tool

### Changed

- `molva transcribe` now runs the core `Pipeline` for file work; the CLI keeps only input discovery, progress and stdout

## [0.2.8] - 2026-11-09

### Added

- Shell completions for `molva` in bash, zsh and fish

- `rules.structure` switches the voice commands for new lines, paragraphs and lists

### Changed

- The Settings tab stays visible under Nix but is fully disabled; `programs.molvai.showSettings` is gone

- `gui.show_managed_notice` and `programs.molvai.showNotice` control the Nix-managed banner; the notice is off by default

- The interface language defaults to `auto` and follows the system

- The generated Nix settings no longer disable `stt.auto_pull`, so a missing model is downloaded at the daemon start

### Fixed

- The GUI wrapper puts `libayatana-appindicator3` on its library path, so the tray icon starts

- The GUI reports the XDG autostart entry written by the Nix module as enabled

- The window has one scrollbar: the page column owns it and the body no longer grows a second one

- The legacy settings migration and legacy key-source tests are gone

## [0.2.7] - 2026-11-08

### Added

- A Nix package for the Tauri window and its tray, built from the same source as the CLI

### Fixed

- The CLI build links `wayland-client`, which the enigo backend needs on Linux

## [0.2.6] - 2026-11-08

### Fixed

- The Linux CUDA release job asks the toolkit action only for `nvcc` and `cudart`, then installs `libcublas-12-6` and `libcublas-dev-12-6` directly. The 0.2.5 attempt still passed `libcublas` through the action, which prefixed it with `cuda-`

### Documentation

- Add the FlakeHub badge to the README

## [0.2.5] - 2026-11-08

### Fixed

- The Linux CUDA release job installs `libcublas` and `libcublas-dev` for CUDA 12.6 instead of the nonexistent `cuda-cublas-12-6` packages

## [0.2.4] - 2026-11-08

### Fixed

- Command mode sends Ctrl+C with the Lua dispatcher of a Hyprland Lua config, and keeps the old `dispatch sendshortcut` form for an older compositor
- A refused copy shortcut no longer ends command mode: the uinput virtual keyboard copies the selection when the compositor does not

## [0.2.3] - 2026-11-08

### Changed

- Update the GUI dependencies to the versions from the Dependabot minor-and-patch group
- Update `jakoch/install-vulkan-sdk-action` to 1.8.1, `Jimver/cuda-toolkit` to 0.2.36 and `softprops/action-gh-release` to 3

## [0.2.2] - 2026-11-08

### Fixed

- Windows path suggestions accept a drive-letter absolute path instead of only a path beginning with `/` or `~`

## [0.2.1] - 2026-11-08

### Fixed

- The macOS and Windows gates now pass: the memory probe test accepts a zero RSS where `/proc` is absent, the Windows MCP fixture escapes backslashes in TOML, and the path tests use the native separator
- `async-executor` is 1.14.1 instead of the yanked 1.14.0

## [0.2.0] - 2026-11-08

### Added

- The GUI tray menu, its tooltip, system notices and setting errors follow the interface language, Russian or English
- A daemon error in the GUI starts with a title in the interface language, and the daemon's English text follows it
- The built-in styles and command mode send the model an English prompt for a phrase in any language but Russian. A Russian phrase keeps the Russian prompt
- A button in the GUI settings writes the Hyprland binds to "~/.config/hypr/molvai.lua" and shows the line that includes the file in "hyprland.lua". The GUI rewrites the file and reloads Hyprland when the hotkeys change
- "molva setup hyprland --write" writes the same file and prints the same line
- A Home Manager module, "homeModules.default": "programs.molvai" installs molva, runs "molva daemon" as a user service and, under Hyprland with a Lua config, writes and includes the binds on each switch
- A Snippets tab: a phrase and its replacement are saved in "[snippets]", and a whole phrase that matches after the dictionary and the rules returns the replacement before the model. Case, punctuation and repeated spaces do not affect the match, and "*" in the phrase matches any text; command mode, file transcription and chunked typing do not use snippets. The list filters its cards by phrase or replacement
- A transcription requested through MCP is tagged MCP in History and in the last phrase panel, while a file transcription keeps the Files tag. An entry made by a snippet is tagged Snippet in the same places

### Changed

- The daemon and the CLI are English only: messages, errors, help text, table headers and "molva doctor" output
- The built-in style names in the CLI are English: Verbatim, Cleanup, Messenger, Email, Code and Formal
- A GUI string with no translation falls back to English instead of Russian
- The Hyprland binds are Lua for Hyprland 0.55 and later, in the GUI and in "molva setup hyprland", in place of the hyprlang lines that the GUI settings showed
- The Hyprland cancel bind lets its key reach the application, so a bare Escape as the cancel key keeps working in other programs
- "molva setup" takes every key from the settings: command mode toggles, as in the GUI, and cancel uses "hotkeys.cancel" in place of a fixed Ctrl+Shift+Escape. The GNOME bind keeps the modifiers of the toggle key
- "molva setup" refuses a hotkey that does not parse, in place of a bind that the compositor rejects
- The documentation is in English
- An empty "command_mode.system_prompt" now means the built-in prompt in the language of the instruction, and it is the new default. A settings file that holds the old default Russian prompt is migrated to the empty value; a prompt the user wrote stays

### Removed

- The flake no longer offers packages for x86_64-darwin: Nixpkgs 26.05 is its last release, and the macOS release build is for Apple silicon only
- clipboard-win 5.4.3 in place of the yanked 5.4.1

### Security

- rustls 0.23.46 in place of 0.23.45 fixes RUSTSEC-2026-0346, RUSTSEC-2026-0347, RUSTSEC-2026-0348 and RUSTSEC-2026-0349
- ctutils 0.4.6, rustls-webpki 0.103.16, webpki-root-certs 1.0.11 and webpki-roots 1.0.11 fix RUSTSEC-2026-0385, RUSTSEC-2026-0382, RUSTSEC-2026-0383 and RUSTSEC-2026-0381

### Fixed

- Paste through the uinput virtual keyboard keeps the physical keyboard layout: the device is switched to the US group for Ctrl+V and back to the group of the real keyboard after the paste. Hyprland is verified; Sway, KDE and GNOME use their own best-effort layout commands
- The window class is read again immediately before the paste, so a terminal receives Ctrl+Shift+V even when the user switches windows during recognition

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

[Unreleased]: https://github.com/rokokol/MolvAI/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/rokokol/MolvAI/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/rokokol/MolvAI/compare/v0.2.11...v0.3.0
[0.2.11]: https://github.com/rokokol/MolvAI/compare/v0.2.10...v0.2.11
[0.2.10]: https://github.com/rokokol/MolvAI/compare/v0.2.9...v0.2.10
[0.2.9]: https://github.com/rokokol/MolvAI/compare/v0.2.8...v0.2.9
[0.2.8]: https://github.com/rokokol/MolvAI/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/rokokol/MolvAI/compare/v0.2.6...v0.2.7
[0.2.6]: https://github.com/rokokol/MolvAI/compare/v0.2.5...v0.2.6
[0.2.5]: https://github.com/rokokol/MolvAI/compare/v0.2.4...v0.2.5
[0.2.4]: https://github.com/rokokol/MolvAI/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/rokokol/MolvAI/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/rokokol/MolvAI/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/rokokol/MolvAI/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/rokokol/MolvAI/compare/v0.1.9...v0.2.0
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
