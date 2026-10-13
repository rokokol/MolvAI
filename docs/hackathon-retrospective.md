# MolvAI at V0ICE: notes on the hackathon of 5–6 September 2026

A detailed retelling of two days: what we did, in what order, where we stumbled and what cured it. We wrote it right after the final, from the session logs, the commit log and `docs/STATUS.md`. The result: first place, 314 of 332 automatic criteria in the last run of the jury checker, releases up to `v0.1.5` for three operating systems

## In short

- The task: an open analogue of Wispr Flow. Hold a key, speak, release, and the text appears in the active field. Processing runs on the laptop, the same on Linux, Windows and macOS
- The solution: a Rust workspace of three crates. `molva-core` (domain, pipeline, infrastructure), `molva` (CLI and daemon), `molva-gui` (Tauri 2 with React). Recognition is whisper.cpp through whisper-rs. Post-processing uses a dictionary, rules and a local model through Ollama. The paste goes through the compositor or keyboard emulation, and the phrase journal is JSONL
- The process: one person plus parallel Claude Code sessions. In the day, six tracks A–F on Opus in separate worktrees. At night, three tracks G–I. In the morning of the second day, one agent on Fable for the keyring. A coordinator merged the branches, ran the gate and fixed the joints
- Test discipline by the `tests` skill: the `just check` gate, the defect list `tests/defects.sh`, and `just falsify`, which breaks each guarantee and requires the suite to notice. By the final there were 742 tests and 136 defect entries, and all of them are caught

## Chronology

### 5 September, day: plan and skeleton

- 15:00 the first commit with the license, then an hour and a half on the plan: 612 jury criteria sorted into sections, and the decisions fixed (Cargo workspace, Tauri 2, JSONL instead of SQLite, PTT plus toggle plus command mode, output mode `auto`, free licenses only)
- 16:57 the skeleton: the trait contracts `AudioSource`, `SttEngine`, `LlmClient`, `TextInjector`, `HotkeySource`, `Journal`, `Clock`, `Notifier` with fakes, the IPC protocol, the journal schema, the config with defaults
- 17:00 six tracks started on Opus: A capture and whisper, B GUI, C Nix, CI, licenses, installation, D rules, dictionary, styles, LLM, journal, statistics, E file decoder, models, WER, bench, F daemon, IPC, paste chain, hotkeys
- 18:13 whisper, cpal, the journal and the pipeline joined into the daemon. 18:20 all tracks in `main`, 610 tests
- Evening: CI brought to green after eight separate failures on Windows and macOS. End-to-end dictation checked on Hyprland. CUDA build through `nix develop .#cuda`. The leaderboard of the organizers, run 07 at 18:31: first place, 272 of 296
- 23:58 a cut in `docs/STATUS.md`, tasks for the night, and the start of tracks G linters and name entropy, H checker signs, sound, `history paste`, I cutting at pauses, draft, language

### The night from 5 to 6 September

- 00:05 the user went to sleep with a request to run the night and to save the limit. On the way, a request for the item "Open panel…" in the tray, done in ten minutes
- 00:20 CI on Windows is red: the GUI IPC client built the socket name itself, and a path from tempdir on Windows is not a pipe name. The mapping of the path to `\\.\pipe\molva-…` moved to the core as one function for the server, the CLI and the GUI
- 01:30 track I merged: the segmenter by pauses, chunks into the worker thread, a draft by default, a cheap choice of language among the allowed ones through `lang_detect` on a short window
- 02:00 track H merged: sound marks, a pause before the paste, cleanup of the model answer, `history paste`, release of the microphone with a measurement, paste into the terminal, single-instance with exit code 7, a quiet tray
- 02:30 a live Ollama. It turned out that `molva transcribe` accepted `--style` and `--no-llm`, but the pipeline did not apply to files at all. We connected the dictionary, the rules and the model. At once a second surprise: `qwen3.5:4b` answered with an empty string, because it spent the whole `max_tokens` on reasoning. `think = false` does not work through the OpenAI-compatible input of Ollama, and `reasoning_effort = none` works. The key `llm.reasoning_effort = auto` appeared
- 03:00 Windows is red again, now the test of the dictionary reload: two file writes in a row get the same timestamp. The file fingerprint is mtime and size, and the test uses `File::set_modified`
- 04:10 track G merged: `clippy::pedantic` on the whole workspace, a ban on `unwrap` and `panic` outside tests, cargo-machete in the gate, 466 abbreviations in names expanded. Two pedantic findings under `cfg` for macOS and Windows came only from the CI matrix
- 04:40 `molva doctor` learned to check the whisper weights by sha256 and the model provider through `GET /models`. The documentation called a nonexistent command `molva model download`, and we fixed it to `molva models pull`
- 07:33 the tag `v0.1.0-alpha`: the CLI archives built, but the GUI did not, because `tauri-action` does not install the npm dependencies. An `npm ci` step and the tag `v0.1.0-alpha.1`, the release in full. At the same time it turned out that my timestamps in STATUS were two hours ahead: I went by feel and not by `date`
- 08:40 the full `just falsify` on the final main: 125 of 127, two entries `stale`, because `list_models` duplicated the lines of the client. We moved out a common `send`, and all entries are caught

### 6 September, morning: the live rehearsal

- 11:50 the user brought the criteria matrix of run 13 (314 of 332, first place, second place 307) and the first bug: on F9 first an "empty audio" error, the second press works
- The analysis took an hour and turned out layered. First I suspected the ALSA error in the log, but it printed on each successful start, and it is noise from the polling of the USB card. Then I reproduced it through the CLI: a start and a stop within milliseconds ended the recording with zero audio, although a short tap must latch hands-free. The test with a fake clock passed. A debug log of the inputs and outcomes of the state machine showed this: `since` was set at the moment of the start command, and the opening of the microphone took up to 300 ms because of the enumeration of all devices with polling. A release 8 ms after the real start looked like a 308 ms hold, above the threshold of 250. Two cures: counting from the really open microphone, and no enumeration of devices for `default`. The start became 23 ms
- While I fixed it, the user reported "reproduced again", although the daemon was still the old one: a rebuild of the CUDA binary takes minutes, and we must warn about this in advance
- The second scenario from the same log: a press during the processing of the previous phrase got "busy" and vanished. First I made a queue, then, on the suggestion of the user, replaced it with real concurrency: the microphone is free right after the release, the next recording runs while the previous one is recognized, and the state machine counts the phrases in processing
- A style change from the panel "does nothing". It turned out to be two defects at once: the daemon did not handle `config.reload` and `style.set` at all, and the panel sent the identifiers `raw` and `email`, which are not in the core. Also `molva config reload` was missing in the CLI, although the hint referred to it
- The original feature, voice editing of the selected text, turned out not to be connected: `copy_selection` was not implemented in the paste chain or in the hyprctl backend. I made it through `sendshortcut CTRL+C` and reading the buffer
- In parallel we closed the cheap criteria: `rustfmt.toml`, the names of the CI steps by tool, `UNIQUENESS.md` in the root, the README sections "Works offline" and "With one command", the screenshots of the panel. Four dependabot PRs with major updates of the actions were closed with a comment
- Journal encryption M-17: XChaCha20-Poly1305, a random nonce, the key in `journal.key` with owner permissions, and the other fields of the line open, so that the statistics work without the key. The user asked what is hard in the keyring, and it turned out that gnome-keyring works on his Hyprland. A separate agent on Fable in its own worktree made `SecretStore` with three implementations: the OS store, a file, and memory for tests, plus `molva secret set/delete/status`. CI ran through a PR, merged on green
- 12:55 the hotkey defaults moved to F9 and Ctrl+Alt: deliberately not ergonomic, but free on any stand. Because of the haste the push went without `cargo fmt` and with a test on the old key, CI went red, and the fix came five minutes later
- 13:15 command mode gave a document with headings and invented content instead of a translation of one word. We rewrote the prompt: the instruction goes before the text, with an explicit ban on headings, markdown and explanations. Checked on five instructions through Ollama
- 13:20 command mode began to get not the selection but the previous content of the buffer, and each edit stacked on the old garbage. The buffer is now cleared before Ctrl+C and polled for up to 600 ms
- 13:38 the tag `v0.1.0`, the release in full by 14:00

### 6 September, day: a check without a human, and the freeze

- The user left and asked me to test the feature for him. I synthesized an instruction through espeak-ng and tried to feed it into a virtual PipeWire microphone in three ways, and none gave a signal in cpal. I opened a GTK4 editor with text, and then it turned out that `sendshortcut` with the keysym `C` does not reach the application with the Russian layout active, while `code:54` works. This also explained why the paste of the user went through the fallback method. We moved the combinations to key codes
- I fed the instruction through a second daemon with a fake recognizer on its own socket: the selected line in the editor was replaced by the model answer, and the "before and after" screenshots went to the README
- 14:15 the user on Windows: the installer puts only the window, without the CLI. The sidecar was not declared in the Tauri config. We moved the declaration to a separate `tauri.sidecar.conf.json` for the release build only, so that the CI gate does not need a ready binary. The tag `v0.1.1` with the CLI inside the packages
- 14:30 automatic download of the weights at the start of the daemon with a progress bar in stderr. A test failed because the automatic download worked for real and downloaded 490 MB to a temporary directory in a minute: tests must not go to the network, so we added an explicit `auto_pull = false`
- 14:50 the first attempt of a CUDA build in the release as a separate optional job, the tag `v0.1.2` seven minutes before the freeze. 15:00 the freeze, main is green

### 6 September, evening: releases after the freeze

- The user tested the packages on Windows and macOS. The link to the weights "does not exist": the address was assembled by hand without a slash, and Hugging Face was unreachable from his network. The workaround was to copy `ggml-small.bin` from the Linux machine. On macOS "the image is damaged": this is Gatekeeper for an unsigned application, and `xattr -cr` cures it. The styles "without the neural network": in the packages the model is off by default, and the threshold is 10 words
- The package versions: `MolvAI_0.1.0` with the tag `v0.1.2`, because the version lived in two places. Now the only source is `Cargo.toml`, and `release.yml` compares the tag with `cargo pkgid`
- CUDA on the GitHub runners: five failures in a row, each on its own layer. The network installer does not register the toolset in MSBuild, and the cure is the Ninja generator and the MSVC environment. In bash from Git, the coreutils `link.exe` is first in PATH, rustc linked with it, and the cure is a step from PowerShell. CUDA 12.6 does not know Visual Studio 2026 on `windows-latest`, `cudafe++` fails even with `-allow-unsupported-compiler`, and the cure is the runner `windows-2022`. Each attempt through a tag cost a version and forty minutes, so we moved the debugging to a separate `cuda.yml` with a manual start. The sixth attempt passed, and an archive of 451 MB with `molva.exe` and three NVIDIA DLLs was attached to `v0.1.5` by hand
- The user after the stand: there was no download bar in the GUI window, only in the stderr of the daemon; the paths to the models are hardcoded; on Windows the behaviour depends on the layout. These are the first tasks for the return

## Pitfalls by topic

### whisper.cpp and models

- Under Nix, `SOURCE_DATE_EPOCH` turns off `GGML_NATIVE`, and ggml builds without SIMD: a 4 s phrase took 84 s. In the devShell, `GGML_NATIVE=ON`. In the package and CI, a portable x86-64-v3 through `GGML_AVX2`, `GGML_FMA`, `GGML_F16C`
- `stt.language = "auto"` on CPU is five times more expensive than a fixed language: whisper detects the language before it applies `audio_ctx`, that is, always on the full 30 s window. The workaround: a short run with a known language and `lang_detect` on the same spectrogram among the allowed languages. It is still 48–149 % more expensive than a fixed one, so the default is `ru`
- `detect_language = true` in whisper.cpp means "detect the language and exit": there will be no text at all
- The encoder always computes a 30 s window: `audio_ctx` sized to the phrase length gives a fourfold gain on CPU
- Cutting at pauses on CPU wins only with chunks of about 3 s: recognition runs at the speed of speech, and with 5 s chunks the queue does not clear before the release. There is a third more total work, but the wait is twice shorter
- The ready bindings of whisper-rs-sys do not match the ABI of MSVC: on Windows libclang generates the bindings, and on Linux and macOS `WHISPER_DONT_GENERATE_BINDINGS=1`
- `qwen3.5:0.8b` answers in half a second but gives a mess and loses words; the default stays `4b`. The `cleanup` style on `4b` dropped forms of address such as "солнышко", and now the prompt lists what counts as a filler and forbids deleting the rest

### Ollama and the language model

- Reasoning models spend the whole `max_tokens` on reasoning and return an empty `content`. `think = false` does not work through `/v1/chat/completions` of Ollama, and `reasoning_effort = "none"` works: 0.6 s instead of 12 s and emptiness
- An empty answer of the model must not wipe the text: the pipeline returns the text after the rules
- Command mode: the instruction must go first and the text second, otherwise a small model takes the fragment for the task and invents a document with headings

### Daemon, hotkeys, microphone

- The hold was counted from the start command, and the opening of the microphone took up to 300 ms because of the enumeration of all devices with polling: a short tap looked like a hold and ended the recording empty. Count from the real start of the capture, and without enumeration for `default`
- A press during processing was lost as "busy". The right answer is not a queue but concurrency: the microphone is free, the jobs for the worker thread go in order anyway, and the state machine counts the phrases in processing
- The daemon did not handle `config.reload` and `style.set`, and the panel sent foreign style identifiers. Test new IPC commands through the daemon handle, not only through the state machine
- The ALSA line `pcm_dsnoop: unable to open slave` in the log at each start is noise from the polling of a USB card that PipeWire holds, not a capture error

### Wayland and paste

- Hyprland leaves the modifier pressed after `sendshortcut`, the client handles the combination only after `sendkeystate up`, and a pause is needed between them
- With the Russian layout active, the keysyms `V` and `C` are not found on the keyboard, and `sendshortcut` silently sends nothing. Set the keys by codes: `code:55` for V, `code:54` for C. Ctrl+A does not reach GTK4 either, so select through Home and Shift+End
- `wl-paste` right after Ctrl+C returns the previous content of the buffer. Clear the buffer before Ctrl+C and poll for up to 600 ms
- The recording overlay is cut on purpose: `gtk-layer-shell` is under GPL

### CI, releases, licenses

- Windows: named pipes only `\\.\pipe\…`, and a path from tempdir maps deterministically to a pipe name. A closed port answers in about a second, and the tests for "unavailable" get a timeout. CRLF in the marker files breaks the harness, and a `.gitattributes` with `eol=lf` is needed. Two file writes in a row get the same timestamp
- macOS: bash 3.2 without `[[ -v ]]`, `declare -A`, `mapfile`
- cargo-deny: `Unlicense` at whisper-rs, `CDLA-Permissive-2.0` at the root certificates, the path dependencies as a wildcard need `publish = false`, gtk-rs 0.18 from Tauri is declared unmaintained. `rustsec/audit-check` needs `checks: write`
- Pedantic clippy under the `cfg` of other operating systems is not visible on Linux, and the findings come only from the CI matrix
- `tauri-action` does not install the npm dependencies, so you need your own `npm ci`. The sidecar is not bundled if it is not declared, and a declaration in the main config breaks the CI gate, where there is no binary yet: a separate config through `--config` for the release only
- The release version must live in one place, and the tag must be compared with it before the build
- CUDA on the Windows runners: Ninja instead of MSBuild, a step from PowerShell, the runner `windows-2022`. Debug through `workflow_dispatch`, not through tags
- The GUI packages are not signed: macOS shows "damaged", and `xattr -cr` cures it. A real signature needs a paid Apple account

### Process and tools

- Parallel tracks in worktrees always conflict in the same places: `mod.rs`, `main.rs`, `Cargo.toml`, `Cargo.lock`, `tests/defects.sh`. The solution is union and `--theirs Cargo.lock` with a rebuild
- A parallel `just falsify` in the same worktree restores the files from git and overwrites the uncommitted edits. Either edits or falsify
- rustfmt wraps long lines, and the `defects.sh` entries with a one-line FIND become `stale` after the formatting
- The jury checker reads the code with an agent by "signs" and measures a mutation score, including the test fixtures. The entropy of identifier names falls because of short names, not long ones: `id`, `err`, `cfg` hurt, and long test names help. We did not reach the threshold of 3.0 bits for a structural reason: half of the names in any project are shorter than seven characters
- The tracks on Opus hit the session limit twice, and the agents resumed through a message that said what had changed in main in the meantime
- Keep track of time only by `date`: the sense of time in a long session drifts by hours
- Before you ask the user to recheck a bug, make sure that the daemon is already restarted on the new build

## Final numbers

- Commits during the hackathon: about two hundred, and none of them is a squash or a force-push
- Tests: 742 in the gate, 136 defect entries, all caught
- The leaderboard, run 13 at 09:44 of the second day: 314 of 332, first place, a lead of 7 criteria over the second
- Latency on CPU with the `small` model: a 15 s phrase waits 6 s after the release instead of 12 with cutting. On an RTX 3060 with CUDA, 0.5–0.9 s per phrase
- The post-processing model `qwen3.5:4b`: about a second per phrase of ten words on GPU

## What to do next

1. A download bar for the weights in the GUI window: a progress event from the daemon to the panel and an indicator in React
2. A catalog of models and a mirror by setting (`stt.models_mirror`), because Hugging Face is sometimes unreachable
3. Windows: check the hotkeys and the paste through enigo with the Russian layout, the same way we fixed hyprctl
4. Signing and notarization of the macOS packages; an installer for Windows with autostart
5. A pre-roll of the capture buffer as an option for those who care more about the first syllable than about a switched-off microphone
6. Deep research of models: GigaAM and Parakeet for Russian, small text-editing models, Silero TE for punctuation without an LLM
7. Filtering of filler through tokenization and SVD as a research branch after a corpus of phrases accumulates in the journal
