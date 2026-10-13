# MolvAI architecture

## Processes

- **`molva daemon`** (`molvad`) owns the microphone, the recognition model and the state. There is one instance per user; a second copy exits with a message
- **`molva`** is the CLI. It sends commands to the daemon over IPC (`record start|stop|toggle|cancel`, `status`), works with the journal and the statistics directly through the file, and transcribes files offline without the daemon
- **`molva-gui`** is Tauri 2: the tray, the settings, the history and the statistics. It is an IPC client of the daemon and starts the daemon if it is not running

The compositor binds on Wayland, evdev, and the global-hotkey plugin in the GUI are three event sources. All of them meet in one place: the IPC commands of the daemon

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
                       ──▶ LlmClient (if the style needs it and the word count is above the threshold)  t_llm
                       ──▶ TextInjector (auto: type / paste)                    t_inject
                       ──▶ Journal.append: one JSONL line
```

The chunks (`stt.chunked`) are recognized while the person speaks. So after the key release only the tail is left, not the whole phrase. `latency_ms.total` counts from the release, and `latency_ms.first_hypothesis` counts from the start of the recording to the first chunk. There can be no chunks at all, for example a short phrase or a source without streaming capture. Then the path is the same as it always was: the whole recording in one run

If the post-processing model fails, the daemon returns the text after the rules. If the paste fails, the text stays in the clipboard and the daemon notifies the user. The phrase goes to the journal in any case, with the `error` field on a failure

## Daemon states

`Idle → Recording → Transcribing → PostProcessing → Injecting → Idle`. `record.start` outside `Idle` returns `busy`. `record.cancel` from `Recording` discards the audio. A key hold shorter than `hotkeys.min_hold_ms` does not create a phrase

## Storage

The only storage is the JSONL journal (`docs/journal-schema.md`). The history, the search, the filters and the statistics are computed from it. Deletion, cleanup and rotation rewrite the file atomically

## Replacing components through settings

- recognizer: `stt.engine`, `stt.model`, `stt.model_path`
- post-processing: `llm.provider`, `llm.base_url`, `llm.model`, `style.default`
- paste: `output.mode`, `output.paste_backend`, `output.type_backend`
- hotkeys: `hotkeys.backend` and the combinations themselves
