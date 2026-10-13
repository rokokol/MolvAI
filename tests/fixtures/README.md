# Fixtures

Audio for the decoder tests (`molva-core/src/infra/audio/decode.rs`), for the pipeline tests and for `molva bench`. The speech files are 16-bit PCM, 16 kHz, mono. This is exactly the format in which the audio goes to whisper.cpp, so the test does not hide an error in the sample-rate conversion

## What is here

| File | What it is | Duration | Purpose |
| --- | --- | --- | --- |
| `privet_ru.wav` | speech, Russian | ~4 s | WER and CER on Russian, golden test of the recognition chain |
| `hello_en.wav` | speech, English | ~4 s | the same on English, check of the automatic language detection |
| `secret_ru_en.wav` | speech, Russian with English insertions | ~4 s | mixed speech (criteria I-01, I-04) |
| `silence.wav` | silence, zeros | 1 s | a phrase without speech must give no text and must not crash |
| `tone.wav` | 440 Hz sine, amplitude 0.5 | 1 s | there is a signal and no speech: check of the `no_speech_threshold` threshold and of the level meter |
| `tone.mp3`, `tone.ogg`, `tone.flac`, `tone.m4a` | 440 Hz sine | 1 s | the decoder returns a signal and not an empty buffer; stereo is mixed down to mono |
| `transcripts.toml` | reference texts | — | what exactly the speech files say; the tests and `bench/manifest.toml` read it |

## License

The author of the project made the speech recordings on 5 September 2026 and releases them into the public domain under [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/deed.en): you can use them in any way and without attribution

Code generated the synthetic files, and no copyright burdens them at all. There is no speech of third parties here, no pieces of other datasets and no recordings from open corpora. This is a deliberate decision: a recording from another person brings that person's license with it, and the `just deny` gate does not reach audio files

## How the tones are generated

```sh
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libmp3lame -b:a 32k tone.mp3
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libvorbis -q:a 0 tone.ogg
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=16000" -ac 1 -c:a flac -compression_level 8 tone.flac
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 1 -c:a aac -b:a 32k tone.m4a
```

The tests themselves create the WAV fixtures for the decoder tests through `hound`

## Add your own recording

```sh
pw-record --rate 16000 --channels 1 tests/fixtures/privet_ru.wav
```

On PipeWire use `pw-record`. On plain PulseAudio use `parec --rate=16000 --channels=1 --file-format=wav`. On ALSA use `arecord -r 16000 -c 1 -f S16_LE`

When you have recorded a file, add a line to `transcripts.toml` with the exact text of the phrase. Without it the file does not take part in the WER count, and the test that reads it will say that the reference is missing

> [!NOTE]
> Record in silence and without processing: noise suppression and normalization make the fixture easier than real audio, and the test starts to pass for reasons that have nothing to do with the code
