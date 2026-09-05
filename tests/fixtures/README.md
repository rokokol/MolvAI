# Фикстуры

Аудио для тестов декодера (`molva-core/src/infra/audio/decode.rs`), тестов конвейера и для `molva bench`. Речевые файлы — PCM 16 бит, 16 кГц, моно: ровно тот формат, в котором звук уходит в whisper.cpp, поэтому тест не маскирует ошибку в приведении частоты

## Что здесь лежит

| Файл | Что это | Длительность | Зачем |
| --- | --- | --- | --- |
| `privet_ru.wav` | речь, русский | ~4 с | WER и CER на русском, golden-тест цепочки распознавания |
| `hello_en.wav` | речь, английский | ~4 с | то же на английском, проверка автоопределения языка |
| `secret_ru_en.wav` | речь, русский с английскими вставками | ~4 с | смешанная речь (критерии I-01, I-04) |
| `silence.wav` | тишина, нули | 1 с | реплика без речи не должна давать текст и не должна падать |
| `tone.wav` | синус 440 Гц, амплитуда 0.5 | 1 с | сигнал есть, речи нет: проверка порога `no_speech_threshold` и измерителя уровня |
| `tone.mp3`, `tone.ogg`, `tone.flac`, `tone.m4a` | синус 440 Гц | 1 с | декодер возвращает сигнал, а не пустой буфер; стерео сводится в моно |
| `transcripts.toml` | эталонные тексты | — | что именно сказано в речевых файлах, читается тестами и `bench/manifest.toml` |

## Лицензия

Речевые записи сделаны автором проекта 5 сентября 2026 года и передаются в общественное достояние на условиях [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/deed.ru): их можно использовать как угодно и без указания авторства

Синтетические файлы порождены кодом и авторским правом не обременены вовсе. Речи третьих лиц, кусков чужих датасетов и записей из открытых корпусов здесь нет — это осознанное решение: чужая запись тянет за собой чужую лицензию, а гейт `just deny` до аудиофайлов не дотягивается

## Как порождены тоны

```sh
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libmp3lame -b:a 32k tone.mp3
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 2 -c:a libvorbis -q:a 0 tone.ogg
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=16000" -ac 1 -c:a flac -compression_level 8 tone.flac
nix shell nixpkgs#ffmpeg --command ffmpeg -f lavfi -i "sine=frequency=440:duration=1:sample_rate=44100" -ac 1 -c:a aac -b:a 32k tone.m4a
```

WAV-фикстуры для тестов декодера создаются самими тестами через `hound`

## Добавить свою запись

```sh
pw-record --rate 16000 --channels 1 tests/fixtures/privet_ru.wav
```

На PipeWire это `pw-record`, на чистом PulseAudio — `parec --rate=16000 --channels=1 --file-format=wav`, на ALSA — `arecord -r 16000 -c 1 -f S16_LE`

Записали — допишите строку в `transcripts.toml` с точным текстом реплики: без неё файл не участвует в подсчёте WER, а тест, который его читает, скажет, что эталона нет

> [!NOTE]
> Записывайте в тишине и без обработки: шумодав и нормализация делают фикстуру легче, чем реальный звук, и тест начинает проходить по причинам, не относящимся к коду
