# What is original here and what is borrowed

A report on the analogues that we studied and on the origin of the MolvAI code. We wrote it so that the question "is this a fork?" has an answer that you can check, not an assurance

## Borrowed code

Exactly one file in the repository was not written for MolvAI and is taken whole:

| File | Source | License |
| --- | --- | --- |
| `scripts/t.sh` | [rokokol/tests-skill](https://github.com/rokokol/tests-skill) | MIT |

This is a test harness: an honest verdict of the run, reading of the log even at exit 0, `falsify`, `flaky`, `bisect`. The author is the same as for MolvAI. We moved the file without changes, together with the `scripts/markers/` directory

Everything else in `crates/` was written in this repository. The libraries are connected as dependencies through Cargo and listed in [THIRD-PARTY.md](../THIRD-PARTY.md). The recognition engine `whisper.cpp` (MIT) is built from source as a dependency and is not copied into the tree

## Studied analogues

Before the design, we read seven projects of the same niche. From each we took the understanding of the task, and from none of them the code

| Project | What it does | What we took as an idea | How MolvAI differs |
| --- | --- | --- | --- |
| [Wispr Flow](https://wisprflow.ai) | a commercial closed product that set the genre: hold a key, speak, the text appears | the push-to-talk interaction model itself and the style of paste into the active field | processing on the machine of the user, open code, no mandatory cloud |
| [Handy](https://github.com/cjpais/Handy) | Tauri + Rust, local whisper.cpp, three operating systems | confirmation that the combination of Tauri 2 and whisper-rs is viable on three platforms | post-processing rules without a model, a term dictionary, a phrase journal, command mode |
| [voxtype](https://github.com/joelkoen/voxtype) | a minimal CLI for Wayland, paste through `wtype` | the paste chain on Wayland and its fallback to the clipboard | the paste chain is chosen automatically and its choice goes to the journal, so it is not a guess |
| [whisrs](https://github.com/kirjavascript/whisrs) | a Rust wrapper over whisper for streaming input | handling of an audio stream that opens only for the time of a phrase | conversion to mono 16 kHz and silence trimming as checkable functions with fixtures |
| [Vocalinux](https://github.com/jatinkumar-dev/Vocalinux) | voice input for Linux in Python, desktop integration | the set of system dependencies that Linux really needs | one statically linked binary instead of a tree of Python packages |
| [OpenWhispr](https://github.com/dictationdaddy/OpenWhispr) | an Electron application, local or through the cloud | the variant "local by default, cloud by explicit setup" | the list of outgoing addresses is fixed in a document and can be checked |
| [whisper-overlay](https://github.com/oddlama/whisper-overlay) | an overlay above the windows with a recognition preview on Wayland | the idea to show the state above everything without taking focus | state in the tray and sound marks instead of an overlay, so that it does not depend on the compositor protocol |

## What we wrote ourselves

- **Post-processing without a language model.** A term dictionary and a set of rules: spoken punctuation, automatic punctuation, removal of filler words and repeats, numbers as digits. A phrase shorter than `rules.llm_min_words` goes through the rules only: a language model is not needed to put in a comma
- **The term dictionary.** Names, abbreviations and jargon go into whisper as a hint and get fixed in the finished text, including by fuzzy matching
- **The phrase journal in JSONL.** Each phrase is a line with the time, the duration, the language, the chosen paste method and its result. [journal-schema.md](journal-schema.md) fixes the schema. The statistics and the "time saved" are computed from it
- **The paste chain.** The paste method is chosen by the environment and the text length, falls back to the clipboard and restores its previous content, and the journal shows each step of the chain
- **`molva bench`.** A run over the fixtures with a count of WER, CER and time: a change in recognition or in the rules is measured by a number, not by a feeling
- **A gate with falsification.** `tests/defects.sh` lists the guarantees that break one by one, and `just falsify` requires the suite to notice. A defect that the suite survived names an unchecked place

## How to check it again

```sh
just falsify                          # the suite catches each broken guarantee
just third-party                      # the list of dependencies and their licenses
grep -rl 'SPDX-License-Identifier' crates | wc -l   # a header in each source file
```

The commit history is linear and was never rewritten. It shows what appeared and when, and `git log --format='%an %s%n%b'` shows the AI disclosure trailers of each commit
