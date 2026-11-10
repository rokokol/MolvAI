# MolvAI architecture

## Processes

- **`molva daemon`** (`molvad`) owns the microphone, the recognition model and the state. There is one instance per user; a second copy exits with a message
- **`molva`** is the CLI. It sends commands to the daemon over IPC (`record start|stop|toggle|cancel`, `status`), works with the journal and the statistics directly through the file, and transcribes files offline without the daemon: `molva transcribe` loads its own engine, which is what an offline batch or CI needs
- **`molva-gui`** is Tauri 2: the tray, the settings, the history and the statistics. It is an IPC client of the daemon and starts the daemon itself when a recording or a file needs one, with the same settings file and the same socket. Quit stops only the daemon that the window started: a process from the Home Manager service or from a terminal survives
- **`molva mcp`** is a stdio MCP server. It has no port, no URL and no autostart: the harness starts it. A transcription goes to the daemon, which the server starts when nobody answers on the socket, so it reuses the recognizer that is already loaded. `mcp.enabled` gates the server

The compositor binds on Wayland, evdev, and the global-hotkey plugin in the GUI are three event sources. All of them meet in one place: the IPC commands of the daemon

## File transcription

A file is one IPC command, `file.transcribe`, not a second process. The daemon reads it with the recognizer and the model it already holds, so the language, the dictionary and the rules are the ones of dictation. The answer has the shape of `molva transcribe --json`. The journal line carries the source of the request: `file` when the GUI or the CLI asked, and `mcp` when an MCP client did, so the statistics count the two apart

The request never moves the state machine. The worker sets the `job` field to `file` while the state stays `idle`, and `molva status` and the panel read that pair as "transcribing a file". Two rules keep one recognizer honest: a phrase in recording or in processing comes first, so a file is refused with `busy` then, and one file job holds the worker at a time, so a second file waits. A recording that starts during a file is accepted and queued behind it, because the audio cannot be repeated

A client decides what to do with the refusal. `molva transcribe` never meets it, because it runs its own engine. The GUI repeats the request for up to half an hour, so a person who dictates in the middle of a file does not lose it, and the MCP server waits thirty seconds before it reports the refusal to the harness

## Layers of `molva-core`

```
domain/   types, traits, pure functions: no dependency on hardware or frameworks
app/      the phrase pipeline, the daemon state machine, config, journal, statistics
infra/    trait implementations: cpal, whisper.cpp, HTTP client, paste, evdev, files
ipc/      the daemon protocol (JSON lines over a local socket)
```

Contracts (`domain`): `AudioSource`, `SttEngine`, `LlmClient`, `TextRule`, `TextInjector`, `HotkeySource`, `Journal`, `Clock`, `Notifier`. Each has a fake in `domain::fakes`, so the pipeline and the daemon are tested without a microphone, a model or a display

## The phrase pipeline

```
hotkey ──▶ RecordStart ──▶ microphone open, level shown in the GUI
           every 250 ms ──▶ Segmenter cuts the stream at pauses
                        ──▶ SttEngine.transcribe chunk       Event::Hypothesis
       ──▶ RecordStop  ──▶ tail → mono 16 kHz
                       ──▶ SttEngine.transcribe tail         t_stt
                       ──▶ join with the chunks
                       ──▶ term dictionary → rules           t_rules
                       ──▶ snippet (whole phrase) ──▶ replacement, no model
                       ──▶ LlmClient (if the style needs it and the word count is above the threshold)  t_llm
                       ──▶ TextInjector (auto: type / paste)                    t_inject
                       ──▶ Journal.append: one JSONL line
```

The rules are never skipped when the model is on: the model receives the text after the dictionary and the rules, and a short phrase, a style without the model or an unavailable model keeps that text unchanged. Voice editing has a separate order:

```
selection ──▶ Ctrl+C through the compositor
          ──▶ spoken instruction ──▶ SttEngine
          ──▶ dictionary (the rules would distort the instruction)
          ──▶ LlmClient with the instruction and the selection
          ──▶ replace the selection with the answer
```

Voice editing is the path behind the `command` hotkey and the `command_mode` settings section: select a paragraph, hold the key, and say what to do with it, for example "сделай короче" or "переведи на немецкий". The normal dictation path types into the active field; voice editing replaces the selected text instead

The chunks (`stt.chunked`) are recognized while the person speaks. So after the key release only the tail is left, not the whole phrase. `latency_ms.total` counts from the release, and `latency_ms.first_hypothesis` counts from the start of the recording to the first chunk. There can be no chunks at all, for example a short phrase or a source without streaming capture. Then the path is the same as it always was: the whole recording in one run

### Snippets

A snippet is a phrase and the text that replaces it. The matching happens after the dictionary and the rules and before the model. A whole phrase that matches a snippet returns its replacement at once: no style, no model, no token cost and no waiting. The replacement is final and is not rewritten

The phrase may contain `*`, which matches any run of characters, including an empty one. So `ответь *` matches `ответь клиенту` and `ответь дорогому клиенту`, and `* клиенту` matches from the other side. The replacement is static: a wildcard changes what triggers the snippet, not what the field receives. An exact phrase wins over a wildcard one, and a wildcard-only phrase is refused because it would replace every dictation

The comparison ignores case, punctuation and repeated spaces, because the rules add punctuation and the recognizer may hear a comma that the phrase does not have. A phrase that merely contains a literal trigger does not match, and an empty phrase or replacement is refused by the validation. Voice editing, file transcription and a phrase typed in chunks do not use snippets: those paths either edit a selection or have no single complete utterance to compare

The settings hold the list:

```toml
[snippets]
enabled = true

[[snippets.items]]
phrase = "Ответь *"
replacement = "Добрый день! Уже подготовил ответ."
```

The Snippets tab of the window edits the same list. The page leaves the settings untouched until Save writes them, and a managed Nix config takes the change as an override in `runtime.toml` beside the managed file

A chunk boundary always falls in the middle of a pause, never on a timer, because a cut inside a syllable makes the model print a wrong word. The pause that counts is `stt.chunk_pause_ms`, 700 ms by default. After ten seconds of one chunk the requirement relaxes to 200 ms: a long sentence may hold no pause of 700 ms, and the wait for one then costs more than a less ideal boundary. There is no forced cut, so a stream without an acceptable pause keeps growing, and such a phrase pays for it with memory and a late first hypothesis

Neighbouring chunks overlap by 150 ms, so a quiet consonant at the boundary reaches both chunks. The overlap and the hint of the previous text also pull the model into repeating the last word of the previous chunk, so a chunk loses one leading word that repeats it. A repetition inside a chunk stays, because the overlap cannot explain it

If the post-processing model fails, the daemon returns the text after the rules. If the paste fails, the text stays in the clipboard and the daemon notifies the user. The phrase goes to the journal in any case, with the `error` field on a failure

Model calls go through one policy (`app::llm_executor`). A local provider takes one request at a time behind a mutex, and a cloud provider answers up to four calls at once. Post-processing of a phrase stays on the worker thread. A style whose prompt asks for a translation gets it from the same model stage: translation is a style, not a second pipeline

## Daemon states

`Idle → Recording → Transcribing → PostProcessing → Injecting → Idle`. `record.start` outside `Idle` returns `busy`. `record.cancel` from `Recording` discards the audio. A key hold shorter than `hotkeys.min_hold_ms` does not create a phrase

## Storage

The only storage is the JSONL journal (`docs/journal-schema.md`). The history, the search, the filters and the statistics are computed from it. Deletion, cleanup and rotation rewrite the file atomically

## Replacing components through settings

- recognizer: `stt.engine`, `stt.model`, `stt.model_path`
- post-processing: `llm.provider`, `llm.base_url`, `llm.model`, `style.default`
- snippets: `snippets.enabled` and the `snippets.items` list, through the Snippets tab or `molva config edit`
- paste: `output.mode`, `output.paste_backend`, `output.type_backend`
- hotkeys: `hotkeys.backend` and the combinations themselves
