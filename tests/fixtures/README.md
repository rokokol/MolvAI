# Аудиофикстуры

Маленькие файлы для тестов декодера (`molva-core/src/infra/audio/decode.rs`) и набора `bench/`.
Все файлы распространяются под [CC0-1.0](https://creativecommons.org/publicdomain/zero/1.0/): речи третьих лиц и защищённых записей здесь нет.

## Синтетический тон

`tone.mp3`, `tone.ogg`, `tone.flac`, `tone.m4a` — синус 440 Гц длительностью ровно 1 секунда.
Сгенерированы ffmpeg из `lavfi` (сигнал вычисляется, ничего не записывается с микрофона), команда для воспроизведения:

```sh
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libmp3lame -b:a 32k tone.mp3
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libvorbis -q:a 0     tone.ogg
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=16000" -ac 1 -c:a flac -compression_level 8 tone.flac
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 1 -c:a aac  -b:a 32k        tone.m4a
```

Тон, а не тишина: по нему видно, что декодер вернул сигнал, а не пустой буфер, и что стерео действительно свелось в моно.
WAV-фикстуры тесты создают сами через `hound`, поэтому в репозитории их нет.

## Речь

`privet_ru.wav` и `hello_en.wav` — 16 кГц моно, записаны автором проекта 05.09.2026.
Эталонные тексты лежат в `transcripts.toml` и используются как `reference` для WER/CER в `bench/manifest.toml`.
