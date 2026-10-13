# Outgoing connections

The full list of addresses that MolvAI calls, and the conditions under which it calls them. We keep the list by hand, and it is a statement that you can disprove: run `molva` under `strace -f -e trace=network` or look in journalctl. Nothing beyond the list here must appear

> [!IMPORTANT]
> There is no telemetry. There are no launch counters, no error reports and no "anonymous statistics". The `privacy.telemetry` setting exists only so that you can state this explicitly and check it

## By default, without setup

| Address | When | What goes out | What comes in |
| --- | --- | --- | --- |
| `huggingface.co` and `cdn-lfs*.huggingface.co` | only at the first installation of a model: `molva models pull` or the first run without weights | HTTP GET for the file `ggml-<model>.bin`, without user identifiers | the weights file, checked against SHA-256 |

Everything else in the default configuration happens on the machine: recording from the microphone, recognition through whisper.cpp, the dictionary, the rules, the text paste and the phrase journal. A phrase does not leave the computer

## Only with explicit setup

| Address | The setting that turns it on | What goes out |
| --- | --- | --- |
| `http://localhost:11434/v1` | `llm.enabled = true` with `llm.provider = "ollama"` (the default value) | the text of the phrase goes to Ollama on the same machine and does not go outside |
| `http://localhost:1234/v1` | `llm.provider = "lmstudio"` | the same, a local LM Studio |
| `https://api.openai.com/v1` | `llm.provider = "openai"` | the text of the phrase goes to a third-party provider |
| `https://openrouter.ai/api/v1` | `llm.provider = "openrouter"` | the same |
| `https://api.groq.com/openai/v1` | `llm.provider = "groq"`, or `stt.engine = "remote-openai"` | the text of the phrase, and for remote recognition the audio recording itself |
| any address | `llm.provider = "custom"` together with `llm.base_url` | what the user configured |

Two settings close the whole right half of the table at once. `privacy.send_to_llm = false` forbids sending text to a language model, whatever the provider. `privacy.no_record_mode = true` also turns off the journal and the history

The settings file does not store API keys. The config holds only the name of an environment variable (`llm.api_key_env`) or an instruction to take the key from the system keystore (`llm.api_key_source`). So you can show the config and commit it

## What never happens

There is no update check over the network. The version is compared only when the user runs `install.sh` or looks at the releases page. The build pulls nothing except crates.io and the npm registry during `cargo build` and `npm ci`, and both steps work from lock files. Autostart opens no network sockets. The IPC of the daemon is a unix socket in the directory of the user, not a TCP port
