# Phrase journal schema

The journal is the only storage of dictation data. The file `journal.jsonl` is in the data directory of the user (`$XDG_DATA_HOME/molva/` on Linux, `~/Library/Application Support/molva/` on macOS, `%APPDATA%\molva\` on Windows), with permissions for the owner only. One line is one phrase, as JSON without line breaks. The file is append-only. Deletion of an entry, cleanup and rotation rewrite the file atomically (a temporary file plus rename)

The reference for the type is `crates/molva-core/src/domain/entry.rs` (`Entry`)

## Line fields

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schema` | int | yes | version of the line schema, now `1` |
| `id` | uuid | yes | unique identifier of the phrase |
| `ts` | string | yes | the moment the phrase finished, RFC 3339, UTC (`2026-09-05T12:00:00Z`) |
| `session_id` | uuid | yes | the daemon run that the phrase belongs to |
| `mode` | `dictation` \| `command` | yes | dictation or editing of the selected text |
| `source` | `mic` \| `file` | yes | microphone or a loaded audio file |
| `app` | string | no | class or name of the active application at the moment of the paste |
| `language` | string | no | language of the phrase (ISO-639-1), detected or set |
| `audio_secs` | float | yes | audio duration after the silence is trimmed at the edges |
| `words` | int | yes | words in the final text (see the counting rule below) |
| `wpm` | float | no | words per minute; absent if `audio_secs < 1` |
| `style` | string | yes | post-processing profile |
| `stt_engine` | string | yes | recognition engine (`whisper-cpp`, `remote-openai`, `fake`) |
| `stt_model` | string | yes | recognition model (`small`, `large-v3-turbo`) |
| `llm_provider` | string | no | provider of the post-processing model (`ollama`, `openai`, …) |
| `llm_model` | string | no | post-processing model |
| `llm_used` | bool | yes | whether the daemon called the model for this phrase |
| `local_llm` | bool | yes | the post-processing model is local (for criterion S-08) |
| `dict_hits` | int | yes | how many dictionary terms were substituted |
| `inject_method` | string | no | the actual paste method (`hyprctl-paste`, `wtype-type`, `enigo-type`, `clipboard-only`) |
| `latency_ms` | object | yes | latencies of the stages, see below |
| `tokens` | object | no | `{ "prompt": int, "completion": int }`: tokens of the post-processing model |
| `error` | string | no | error text, if the phrase was not delivered |
| `text_raw` | string | no | raw text of the recognizer; absent in privacy mode |
| `text_final` | string | no | text after the dictionary, the rules and the model; absent in privacy mode |
| `audio_path` | string | no | path to the saved audio, if storage is on |

### `latency_ms`

| Field | Required | Meaning |
|---|---|---|
| `stt` | yes | recognition |
| `rules` | yes | dictionary and rules without the model |
| `llm` | no | post-processing by the model (absent if the daemon did not call the model) |
| `inject` | no | paste into the active window (absent if an error occurred before the paste) |
| `total` | yes | from the stop of the recording to the end of the paste |
| `first_hypothesis` | no | from the start of the speech to the first hypothesis with streaming preview |
| `stop_after_release` | no | from the key release to the actual stop of the recording |

## Calculation rules

- **A word** is a sequence of non-whitespace characters that contains at least one letter or digit. Lone punctuation marks and dashes do not count as words
- **WPM** = `words / audio_secs * 60`. `audio_secs` is the duration after the silence is trimmed at the start and at the end. Pauses inside the phrase are part of the duration. For phrases shorter than one second, `wpm` is not computed
- **The average speed** for a session, a day and all time is weighted by time: `sum(words) / sum(audio_secs) * 60`, and not the arithmetic mean of `wpm`
- **The personal record** is the maximum `wpm` among the phrases with `audio_secs >= 3` and `words >= 5`
- **A session** is one daemon run (`session_id`). When the gap between phrases is more than 30 minutes, the statistics also split the session

## Privacy mode

With `journal.include_text = false` the line is written without `text_raw`, `text_final` and `audio_path`. All metrics stay. With `privacy.no_record_mode = true` the journal is not written at all

## Example

```json
{"schema":1,"id":"6f1c…","ts":"2026-09-05T12:00:00Z","session_id":"a1b2…","mode":"dictation","source":"mic","app":"kitty","language":"ru","audio_secs":4.0,"words":10,"wpm":150.0,"style":"cleanup","stt_engine":"whisper-cpp","stt_model":"small","llm_used":false,"local_llm":true,"dict_hits":0,"inject_method":"hyprctl-paste","latency_ms":{"stt":540,"rules":2,"inject":120,"total":700},"text_raw":"привет мир","text_final":"Привет, мир."}
```
