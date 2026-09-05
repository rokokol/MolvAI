# MolvAI

[![ci](https://github.com/rokokol/MolvAI/actions/workflows/ci.yml/badge.svg)](https://github.com/rokokol/MolvAI/actions/workflows/ci.yml)
[![deny](https://github.com/rokokol/MolvAI/actions/workflows/deny.yml/badge.svg)](https://github.com/rokokol/MolvAI/actions/workflows/deny.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Открытый системный голосовой ввод: зажал клавишу, молвил, отпустил — текст появился в активном поле любого приложения. Обработка на вашем компьютере, одинаково на Linux, Windows и macOS

> [!NOTE]
> Проект создаётся на хакатоне V0ICE (Казань, 5–6 сентября 2026) и находится в разработке. Что уже работает, а что отрезано — в [docs/STATUS.md](docs/STATUS.md); разделы README заполняются по мере появления функций

## Как это работает

- **Захват** — микрофон через `cpal`, поток открывается только на время записи, звук приводится к моно 16 кГц
- **Распознавание** — `whisper.cpp` локально (CPU, CUDA, Vulkan или Metal), движок заменяется настройкой
- **Постобработка** — словарь терминов и правила без модели; при желании языковая модель через любой OpenAI-совместимый API, Ollama по умолчанию
- **Вставка** — эмуляция клавиатуры или вставка через буфер обмена с его восстановлением; способ выбирается автоматически и фиксируется в журнале

Подробнее — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), схема журнала реплик — [docs/journal-schema.md](docs/journal-schema.md)

## Установка

Самый короткий путь — попросить ИИ-агента: откройте Claude Code в каталоге репозитория и молвите "прочитай docs/install.md и установи MolvAI". Агент определит вашу систему, композитор и звуковой сервер, поставит зависимости, скачает модель и настроит клавиши — [docs/install.md](docs/install.md) написан как промпт именно для этого

### С Nix

```sh
nix run github:rokokol/MolvAI -- --help   # собрать и запустить не клонируя
```

```sh
nix develop                        # тулчейн и системные библиотеки, ничего в систему не ставится
cargo build --release --bin molva
./install.sh                       # бинарник в ~/.local/bin, права администратора не нужны
```

### Без Nix

```sh
./install.sh --check   # что не хватает и какими командами это ставится в вашем дистрибутиве
```

Скрипт ничего не устанавливает сам: он печатает точные команды для Debian, Fedora, Arch, openSUSE, macOS и Windows, а решение принимает человек. Поставили — запустите `./install.sh` ещё раз

### Windows

Своего `install.ps1` пока нет, и заглушки вместо него тоже нет — честнее сказать прямо, чем положить в репозиторий скрипт, который ничего не делает. Собирайте `cargo build --release --bin molva` и кладите `target\release\molva.exe` куда удобно, либо возьмите готовый архив со страницы релизов; агент из [docs/install.md](docs/install.md) проводит через это по шагам

### Удаление

Снимается ровно то, что было установлено, по манифесту:

```sh
./install.sh --uninstall               # спросит про журнал реплик и настройки
./install.sh --uninstall --purge       # снести вместе с историей
./install.sh --uninstall --keep-history
```

## Быстрый старт

```sh
molva model download small   # веса с Hugging Face, проверяются по SHA-256
molva devices                # список микрофонов
molva test-inject            # проверить, что текст попадает в активное поле
molva daemon                 # демон, слушающий горячие клавиши
```

Горячие клавиши в Hyprland — две строки в `~/.config/hypr/hyprland.conf`, где `bindr` срабатывает на отпускание:

```
bind  = , Control_R, exec, molva start
bindr = , Control_R, exec, molva stop
```

В GNOME и KDE те же две команды вешаются на пользовательскую комбинацию в настройках клавиатуры, на X11 без окружения годится `sxhkd`, а на Windows и macOS комбинацию регистрирует само приложение

## Настройки

Файл `~/.config/molva/config.toml` (на Windows и macOS — соответствующий каталог настроек пользователя) создаётся при первом запуске и читается человеком. Пустой файл — валидный конфиг: любое пропущенное поле берётся из значений по умолчанию, поэтому в файле имеет смысл держать только то, что вы меняли

| Секция | Ключ | По умолчанию | Что делает |
| --- | --- | --- | --- |
| `audio` | `device` | `default` | микрофон из `molva devices` |
| `audio` | `gain` | `1.0` | усиление входного сигнала |
| `audio` | `max_duration_secs` | `600` | предел длины одной реплики |
| `audio` | `trim_silence` | `true` | обрезать тишину по краям записи |
| `audio` | `silence_threshold_db` | `-45.0` | порог, ниже которого сигнал считается тишиной |
| `audio` | `vad_min_pause_ms` | `1500` | пауза короче этой не режет реплику |
| `audio` | `noise_suppression` | `false` | шумоподавление |
| `audio` | `sounds` | `true` | звуковые метки начала и конца записи |
| `audio` | `warn_zero_level` | `true` | предупреждать о нулевом уровне сигнала |
| `stt` | `engine` | `whisper-cpp` | `whisper-cpp` локально или `remote-openai` |
| `stt` | `model` | `small` | какие веса использовать |
| `stt` | `model_path` | пусто | путь к файлу весов; пусто — каталог моделей по умолчанию |
| `stt` | `language` | `auto` | код ISO-639-1 или `auto` |
| `stt` | `allowed_languages` | `["ru", "en"]` | среди каких языков выбирать при `auto` |
| `stt` | `threads` | `0` | потоков распознавания; `0` — все логические ядра |
| `stt` | `unload_after_secs` | `600` | через сколько выгрузить модель из памяти |
| `stt` | `no_speech_threshold` | `0.6` | порог, выше которого фрагмент считается не речью |
| `dictionary` | `path` | пусто | словарь терминов; пусто — `dictionary.toml` рядом с конфигом |
| `dictionary` | `fuzzy` | `true` | нечёткое сопоставление терминов |
| `dictionary` | `in_prompt` | `true` | передавать термины подсказкой в whisper |
| `rules` | `enabled` | `true` | постобработка правилами вообще |
| `rules` | `spoken_punctuation` | `true` | «запятая» голосом превращается в запятую |
| `rules` | `auto_punctuation` | `true` | расставлять пунктуацию самостоятельно |
| `rules` | `remove_fillers` | `true` | вычищать слова-паразиты |
| `rules` | `remove_repeats` | `true` | схлопывать повторы |
| `rules` | `numbers_as_digits` | `true` | числительные цифрами |
| `rules` | `llm_min_words` | `10` | реплики короче обрабатываются без языковой модели |
| `llm` | `enabled` | `false` | звать ли языковую модель вообще |
| `llm` | `provider` | `ollama` | `ollama`, `lmstudio`, `openrouter`, `groq`, `openai` или `custom` |
| `llm` | `base_url` | `http://localhost:11434/v1` | адрес OpenAI-совместимого API |
| `llm` | `model` | `qwen3.5:4b` | модель постобработки |
| `llm` | `api_key_env` | `OPENAI_API_KEY` | имя переменной окружения с ключом |
| `llm` | `api_key_source` | `keyring` | `keyring` или `env`; сам ключ в файле не хранится |
| `style` | `default` | `cleanup` | стиль обработки текста по умолчанию |
| `style` | `by_app` | пусто | класс окна → свой стиль |
| `output` | `mode` | `auto` | `auto`, `paste`, `type` или `clipboard` |
| `output` | `auto_type_max_chars` | `200` | длиннее этого `auto` переключается на буфер обмена |
| `output` | `restore_clipboard` | `true` | вернуть прежнее содержимое буфера после вставки |
| `output` | `type_delay_ms` | `4` | задержка между символами при эмуляции ввода |
| `output` | `notify_on_fallback` | `true` | сообщать, когда способ вставки пришлось сменить |
| `hotkeys` | `backend` | `auto` | `auto`, `external`, `evdev` или `gui` |
| `hotkeys` | `push_to_talk` | `RightCtrl` | клавиша удержания |
| `hotkeys` | `toggle` | `Ctrl+Shift+Space` | включить и выключить запись нажатием |
| `hotkeys` | `command` | `Ctrl+Shift+Alt+Space` | режим команд над выделением |
| `hotkeys` | `min_hold_ms` | `200` | удержание короче не создаёт реплику |
| `journal` | `enabled` | `true` | вести журнал реплик |
| `journal` | `include_text` | `true` | `false` — строка журнала без текста реплики |
| `journal` | `max_entries` | `10000` | сколько реплик хранить |
| `privacy` | `send_to_llm` | `true` | разрешена ли отправка текста языковой модели |
| `privacy` | `no_record_mode` | `false` | выключает журнал и историю целиком |
| `privacy` | `telemetry` | `false` | телеметрии нет; ключ существует, чтобы это заявить явно |
| `autostart` | `enabled` | `false` | запускать при входе в систему |
| `log` | `level` | `info` | `error`, `warn`, `info`, `debug` или `trace` |

Правка файла применяется без пересборки, неверное значение даёт ошибку с путём к файлу и именем ключа. Полный список полей — в `crates/molva-core/src/config.rs`

## Замена модели распознавания

Модель — это один файл весов, и меняется она одной настройкой:

```sh
molva model list              # что доступно и что уже скачано
molva model download large-v3-turbo
```

```toml
[stt]
model = "large-v3-turbo"
```

`base` быстрее и заметно хуже, `small` — компромисс по умолчанию, `large-v3-turbo` точнее и требует больше памяти. Свои веса подключаются через `stt.model_path`: подойдёт любой файл в формате `ggml`, который понимает whisper.cpp

Веса в репозиторий не входят и лицензией проекта не покрываются — их условия собраны в [docs/model-licenses.md](docs/model-licenses.md), а адреса, откуда они скачиваются, перечислены в [docs/outgoing-endpoints.md](docs/outgoing-endpoints.md) вместе со всеми остальными исходящими соединениями

## Оригинальная фича

> [!NOTE]
> Раздел заполняется: функция реализуется на хакатоне, и описание появится здесь вместе с ней, а не раньше

## Тесты

Одна команда, обычный Cargo, без Nix и без `just`:

```sh
cargo test --workspace --no-fail-fast
```

Тесты не требуют микрофона, модели и сети: железо подменяется фейками, аудио берётся из `tests/fixtures/`. Крейту GUI нужны системные библиотеки Tauri; если их нет, проверяйте ядро и CLI отдельно:

```sh
cargo test -p molva-core -p molva --no-fail-fast
```

Системные зависимости для сборки на Debian и Ubuntu: `cmake libasound2-dev libxkbcommon-dev libwayland-dev` для ядра и CLI, плюс `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev` для GUI. Полный список для других дистрибутивов печатает `./install.sh --check`. Тулчейн Rust закреплён в `rust-toolchain.toml`, `rustup` подхватит его сам

Тот же прогон с честным вердиктом (лог читается даже при exit 0) и остальные проверки — через `just`:

```sh
just test     # cargo test через scripts/t.sh
just check    # fmt, clippy, тесты, SPDX-заголовки, отсутствие заглушек
just falsify  # сломать каждую гарантию из tests/defects.sh и потребовать, чтобы сьют заметил
```

## Разработка

```sh
nix develop   # тулчейн и системные библиотеки
just check    # fmt, clippy, тесты, SPDX-заголовки, отсутствие заглушек
```

Тесты идут через `scripts/t.sh` — харнесс из [скилла tests](https://github.com/rokokol/tests-skill): статус прогона честный, лог читается даже при exit 0

```sh
just falsify        # сломать каждую гарантию из tests/defects.sh и потребовать, чтобы сьют заметил
just flaky 10       # десять прогонов подряд: расходятся ли результаты на одном коде
just cov            # покрытие в coverage.lcov
just deny           # лицензии зависимостей и уязвимости
just third-party    # пересобрать THIRD-PARTY.md
just sbom           # ведомость состава в формате CycloneDX
molva bench         # WER, CER и время на фикстурах из tests/fixtures
```

CI вызывает те же рецепты `just`, а не копии их команд, поэтому зелёный прогон в Actions означает ровно то же, что зелёный `just check` на вашей машине

Чем этот проект отличается от изученных аналогов и какой код в нём заимствован — [docs/uniqueness.md](docs/uniqueness.md)
