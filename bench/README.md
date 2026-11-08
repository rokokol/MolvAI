# MolvAI local checker

The organizers do not provide a checker, so MolvAI measures its own quality and speed with one command

```sh
molva bench
```

The command reads `bench/manifest.toml`, runs each case through the recognition engine and prints a summary: WER and CER per case and on average, the p50/p95/p99 latencies and the p99/p50 ratio, the total audio duration, and the RTF (the ratio of recognition time to recording duration)

A bad WER is not a launch error but a measurement result, so the exit code stays zero. A non-zero code (6) means the set is missing or unreadable

## What it looks like

```sh
molva bench --summary            # a table, the default behaviour
molva bench --json               # the full report in JSON, for CI and charts
molva bench --repeat 5           # five runs of each case: the latency spread is visible
molva bench --engine fake        # a pipeline self-test without model weights
molva bench --set my-set/        # another set
molva bench --latency            # one 15 s file: recognition, post-processing and memory
molva bench --latency --style cleanup --repeat 5 --audio bench/latency-15s.wav
```

Data goes to stdout. Progress and warnings go to stderr, so `molva bench --json > report.json` gives clean JSON

## The set format

A set is a directory with `manifest.toml` and audio files. Audio paths are relative to the set directory

```toml
[[case]]
id = "ru-short-phrase"          # optional; the file name by default
audio = "audio/привет.wav"        # any format: wav, mp3, ogg, flac, m4a
reference = "Привет, как дела?"   # the reference text
language = "ru"                   # optional; without it the language is detected automatically
```

The checker compares the reference with the transcript after normalization: case, punctuation and "ё" do not count as errors, because a person cannot tell them apart by ear. An empty `reference` means "there is no speech here": any transcript of such a case is a hallucination, and the WER is one

## How to add your own case

1. Record a phrase and put the file in the set, for example in `bench/audio/`
2. Add a `[[case]]` block with the path to the file and the exact text of what you said
3. Run `molva bench` and look at the row of your case in the table

Write the reference as a person would write what they heard: with normal punctuation and capital letters. You do not need to convert it to a "machine" form, because the checker does that

## What the set already has

The speech fixtures of the repository (`tests/fixtures/`) and `silence.wav`, one second of silence. The silence is not there for completeness. It is the only case that catches model hallucinations on empty input, and it also checks that the pipeline does not crash on a recording without speech

## Repeatability

The score (`wer_avg`, `cer_avg`, the number of failed cases) depends only on the audio, the references and the engine, so two runs in a row give the same number. The latencies, of course, vary from run to run and are not part of the score. Look at them separately, by p50/p95/p99


## Latency and memory

`--latency` measures one audio file instead of the WER set. It reports the median recognition
time, the extra time of the post-processing stage, and the peak resident and video memory of
the process tree. The recognizer may run in the process or in a child (`transcribe-cli`), and
the probe covers both; `nvidia-smi` gives the VRAM when it is available, and the result says
`n/a` rather than inventing a zero

```sh
molva bench --latency --audio bench/latency-15s.wav --repeat 5
molva bench --latency --audio bench/latency-15s.wav --repeat 5 --no-postprocess
molva bench --latency --audio bench/latency-15s.wav --repeat 5 --style cleanup
```

The checked-in `latency-15s.wav` is the short fixture repeated to 15 seconds. It measures time
and memory, not quality: there is no reference and the repeated words do not matter. A result
on a Ryzen 7 5700X, 16 threads, with the GigaAM Q4_K_M weights and the `cleanup` style:

```text
recognition p50 3625 ms  (4.14x realtime)
post-processing p50 5798 ms  (total 9423 ms)
peak RAM 252 MiB  peak VRAM 0 MiB
```

The cloud model decides the post-processing time, so it is not a constant. The CPU-only
GigaAM build uses no VRAM; the field is `0 MiB` because no compute application from this process tree held a GPU
allocation; a machine without `nvidia-smi` gets `n/a` instead.
