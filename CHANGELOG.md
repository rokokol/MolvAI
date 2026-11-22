# Changelog

This file follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

The entries up to 0.1.9 were written after the fact, from the release tags and the git history

## Unreleased

### Added

- Two spend charts in Statistics, one for recognition and one for the model, each on its own scale in US dollars over the 7, 30 or 90 days the words chart covers. A day whose provider named no price becomes a gap in the line of that service and leaves the other line whole, so an unpaid bill is never drawn as zero

- Pauses in a file follow speech instead of loudness: Silero VAD marks the frames that hold a voice, so a lecture over music, hiss or a fan is still cut between utterances. The switch of "Cut at pauses" keeps its meaning, the weights are 884 KB and MIT and ride inside the binary, and the trailing background stays in the last chunk instead of becoming one more request. Live dictation keeps the level threshold

- A settings switch for the live typing of the chunks. With `stt.streaming_inject`, which is on by default now, each ready chunk goes into the input field while the person still speaks, with its rules applied, and only the tail follows after the release

### Changed

- The live typing of the chunks stays with the styles that call no model: the answer of the model would repeat what the field already holds, so Cleanup and Formal deliver the whole phrase after the release, as before, while «Как сказано» and the custom styles without a model type it as it is recognized

- Journal timestamps come from `jiff` instead of `chrono`. The format stays RFC 3339 in UTC, and a line written by an older build opens unchanged. A fractional part that ends in zero digits now drops them, so `…682721110Z` reads `…68272111Z`, and both name the same instant. `chrono` leaves the build, and its unmaintained advisory leaves the dependency gate

## [0.4.0] - 2026-11-17

### Added

- Recognition through an OpenAI-compatible cloud service: `stt.engine = "remote-openai"` sends the recording to `stt.remote.base_url` (OpenAI, Groq, OpenRouter or a server of your own) and reads the transcript back with its timecodes. `stt.remote.timeout_secs` caps one request, `molva secret set stt` writes the key, and `molva secret status` shows the entry beside the model key and the journal key

- OpenRouter transcription through its `/audio/transcriptions` endpoint, with the routing the model section already carries: `stt.remote.zdr`, `data_collection`, `provider_order` and `allow_fallbacks` go in the `provider` part of the request, and the hint reaches the service as separate `keyterms` instead of a prompt. OpenAI and Groq take the hint as one prompt and receive no routing at all, because they read neither

- The cloud recognizer offers the providers whose API transcribes as presets beside a custom endpoint: OpenAI, OpenRouter with `microsoft/mai-transcribe-2` and Groq, each filling the address, the model and the key variable. The address, the key and the knobs of the request sit behind one summary, as in the model section, and the summary of a custom endpoint arrives open

- The remote recognizer carries the knobs of the post-processing model: `stt.remote.temperature` (0 by default, so a phrase reads the same way every time), `stt.remote.prompt` for names and jargon (empty means no prompt is sent), and `stt.remote.max_retries`, which repeats a request that failed on a timeout, a rate limit or a provider error and never one that failed on the key. The Settings page shows them beside the key, in the order of the model section

- The consent to a cloud provider now covers the audio as well, checked on the address of the request: without `privacy.cloud_consent` a remote service receives nothing, while a loopback address needs no consent because the recording stays on the machine

- `stt_cost_usd` and `stt_unpriced` in the journal, and a recognition line in `molva stats` and the Statistics tab: the price a provider reports is summed, and the phrases whose provider reported none are counted apart, so the sum is never mistaken for the whole bill

- **Breaking:** `file.cancel` names the job it stops, so a cancel that arrives while a file still waits for the worker stops that file and not the work that holds the recognizer. An identifier that no running job carries is answered with `cancelled: false`, and a job that already ended leaves its token behind for nobody

- The cancel of a file transcription reaches the recognizer process, and it does so on every platform: the job owns the process instead of its number, so a job that already finished cannot have its number reused by another program, and Windows stops killing nothing. A job that was cancelled before the recognizer starts never starts it

- **Breaking:** the settings that a running process cannot take now say so. `config.reload` answers `restart_required` for `privacy`, `journal`, `llm`, `stt`, `audio`, `dictionary`, `rules`, `output`, `mcp`, `gui`, `log` and for a changed hotkey. Only the style and the thresholds of the hotkeys apply live, and the answer comes after the worker applied them, never before

- **Breaking:** a save that needs a restart leaves the file on disk and the process on its old settings, and the window says which sections are behind. A daemon this window did not start is not stopped any more, and a declared `molva-daemon` service is not replaced by a sidecar

- **Breaking:** the MCP server is off until `mcp.enabled = true`. A harness that relied on the old default sees a refusal that names the switch

- **Breaking:** the journal refuses to open when the key is missing and the journal already holds encrypted text. Before, a new key was created and the old entries became unreadable

- **Breaking:** a daemon whose model needs `transcribe-cli` no longer starts when the program is absent. The error names the three ways out, and `molva doctor` has a line for the program

- **Breaking:** the queue of file transcriptions lives in the window and is not restored after a restart. The results stay reachable through the history, wherever the journal kept them, and the open copy that older versions left in the webview storage is removed on the first start

- **Breaking:** an autostart entry starts the window minimized to the tray only when it carries `--autostart`, so entries written by an older version must be switched off and on once. The behaviour itself is the new `gui.start_in_tray` setting

- The journal keeps its limits: `max_entries` and `max_size_mb` now rotate the file, and one process no longer loses an entry that another appended at the same moment. The lock, the rewrite and the header creation share one protocol. A rotation cuts a tenth below the limit, so the limits read as "not more than" and a full journal is not rewritten with every phrase

- `gui.start_in_tray` and the `--autostart` argument, so a window that starts with the session waits in the tray instead of opening in front of the person

- `hotkeys.tap_toggles` in the settings window, beside the keys it belongs to

- `output.paste_backend` and `output.type_backend` now move the named method to the front of the chain instead of being ignored, and an unknown name is refused when the settings are loaded

- `hotkeys.backend = "evdev"` now starts a source of its own in the daemon on Linux, so a machine without compositor binds has a working hold key

- `molva doctor` names `transcribe-cli`, and the search for it looks beside the `molva` binary as well as in `PATH`, which is where an AppImage and a package put it

### Changed

- A file cut at its pauses goes to the cloud four chunks at a time: an engine that answers over the network reports a replica, so a ten-minute file waits for the number of chunks divided by four instead of one after another. The texts keep the order of the file, the price is the sum of the chunks, and a cancel stops sending. An engine in this process reports no replica and keeps one chunk at a time

- The history list paints a window of the journal instead of the whole file: a journal of 10 000 phrases puts ten rows in the DOM, the rest of the height is padding that the measured rows fill in as they scroll. A search keystroke redraws a screenful rather than a year of dictation

- The settings, statistics, files and snippets views arrive in their own chunk: the window opens on the dashboard with 328 KB of script instead of 404 KB, and a view downloads when it is opened

- The prose face of the window is cut out of the vendored one: the letters of Russian and English, their punctuation, the arrows and the symbols of the interface travel in a 147 KB file instead of the whole 459 KB face, and the Greek and mathematical glyphs stay in a second file that the browser asks for only when such a character reaches the screen. No glyph is dropped, the kit's file stays the source, and `just subset-font-check` proves the committed split still matches it

- The settings page reads the catalog, the providers, the bind file, the style names, the paths and the autostart in one command instead of eight, so one screen costs four trips through the IPC layer

- The consent to a cloud provider is checked at the HTTP boundary on the address of the request itself, so the file transcription, the voice command, the probe and every other entry go through one gate. Without consent the client follows no redirect and no proxy, and `localhost` is resolved to a loopback address only

- Encryption of the journal now covers the text of the segments of a file transcription, and a delete, a rotation or a clear keeps the existing ciphertext instead of writing plaintext back

- The MCP server reads the settings again for every request, so a changed journal path, a new key or a disabled server reaches a session that is already open

- `molva transcribe --language` now sets the language of that one request instead of being parsed and forgotten

- An offline `molva transcribe` writes no journal when the journal is disabled, and creates neither the file nor the key

- A settings file that holds an API key is written with permissions for its owner only, and the temporary file it is staged in can no longer be predicted or replaced

- A reload also rereads the dictionary, so a new term works without a restart

- A damaged journal line is moved to `<journal>.broken` exactly once: the move happens under the exclusive lock, a line already there is not written again, and the journal drops the line it moved. A read that finds the writer busy reports the damaged lines and leaves them for the next write instead of waiting

- The window stays responsive on a large history: the journal commands run off the window thread, the rows of the history and of a transcript are memoised, the transcript is parsed once per result instead of once per position change, and a long history is drawn in batches

- `molva doctor` names the line `model` instead of `whisper model`, because the default weights belong to GigaAM

- A failed command prints its whole cause chain, so a refused journal names the key it is missing instead of only the path

### Fixed

- A file transcript with timecodes showed the text before the dictionary, the rules and the model: the segments came from the recognizer untouched while only the whole text was post-processed, so the Files tab looked as if post-processing never ran. The segments now carry the processed text, and a rewritten passage whose boundary no longer exists keeps the combined interval rather than a guessed timecode

- `molva doctor` names the cloud recognizer instead of the weights: the address, the model, and the way out when the key or the consent is missing. A key that lives in the environment is named rather than reported missing, because the daemon reads its own environment and not the shell's

- A batch export of two files with the same name writes both results instead of overwriting the first

- A phrase that is added to an encrypted journal is encrypted even when its text starts with the ciphertext marker

- The tray tells the person what to do next when a save needs a restart, on the platform that is running: the service manager where a unit exists, the window everywhere else

- The installation guide and the installer name the real command for a model and explain that the default model needs `transcribe-cli`

## [0.3.2] - 2026-11-11

### Added

- Snippets lead the site: a page of their own with the matching rules, wildcards, the settings file and the limits, a section on the home page and on the features page, and a place in the navigation of every page

- The site gives voice editing a page of its own: select text, say "translate to German" or "add emoji", and the selection is replaced. Styles got a section beside it

- File transcription has its own section on the home page, with a timecoded transcript and a search hit, and the feature sections now sit under the screenshots rather than above them

- The statistics count what the model calls cost. The provider reports the charge of each request, so the journal keeps it per phrase and no price table goes stale; `molva stats` and the Statistics tab show the sum, and name the calls that reported no charge instead of counting them as zero

- A transcription through the MCP server enters that same sum, because the journal is the one place a phrase is written and the statistics read it without asking where the phrase came from. The report names the MCP share beside the total, so the sum can be checked instead of trusted

- A retry is billed like the first attempt, so a phrase that was answered on the second call counts both, and a phrase whose calls all failed keeps what they cost next to its error. An answer the provider returned with an error object inside it no longer loses the charge: the client hands the empty text back with the provider's reason, and the pipeline retries while the journal keeps the money

### Changed

- Voice editing is the name of the feature everywhere: the site, the GUI hotkey label, the documentation and the error the daemon returns. `command` stays the name of the hotkey key and `command_mode` the name of the settings section, because a rename there would break every existing configuration

- Whether a model is local now follows the address rather than the provider name. Ollama Cloud answers at `https://ollama.com/v1` under the same `ollama` id, so a cloud model was counted as a local one and its calls as free; the name now only supplies the default URL, and an empty address still means the local server

- The Statistics tab reads the summary of the core instead of its own copy: the copy drifted, and the window showed "not reported" for a priced call because the fields it asked for were not in the answer. The CSV export of the window uses the same table as `molva stats export`, which is one answer instead of two

- The gate checks the Nix layout and the travelling Nix checker, so a formatting slip no longer reaches CI: the checker runs on a worktree, because Nix copies a path flake whole and the build directory is a hundred gigabytes

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
