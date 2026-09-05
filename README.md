# MolvAI

[![ci](https://github.com/rokokol/MolvAI/actions/workflows/ci.yml/badge.svg)](https://github.com/rokokol/MolvAI/actions/workflows/ci.yml)
[![deny](https://github.com/rokokol/MolvAI/actions/workflows/deny.yml/badge.svg)](https://github.com/rokokol/MolvAI/actions/workflows/deny.yml)
[![license](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

Открытый системный голосовой ввод: зажал клавишу, сказал, отпустил — текст появился в активном поле любого приложения. Обработка на вашем компьютере, одинаково на Linux, Windows и macOS

> [!NOTE]
> Проект создаётся на хакатоне V0ICE (Казань, 5–6 сентября 2026). Разделы README заполняются по мере появления функций, текущее состояние — в `docs/STATUS.md`

## Как это работает

1. **Захват** — микрофон через `cpal`, поток открывается только на время записи, звук приводится к моно 16 кГц
2. **Распознавание** — `whisper.cpp` локально (CPU, CUDA, Vulkan или Metal), движок заменяется настройкой
3. **Постобработка** — словарь терминов и правила без модели; при желании языковая модель через любой OpenAI-совместимый API, Ollama по умолчанию
4. **Вставка** — эмуляция клавиатуры или вставка через буфер обмена с его восстановлением; способ выбирается автоматически и фиксируется в журнале

Подробнее — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), схема журнала реплик — [docs/journal-schema.md](docs/journal-schema.md)

## Разработка

```sh
nix develop   # тулчейн и системные библиотеки
just check    # fmt, clippy, тесты, SPDX-заголовки, отсутствие заглушек
```

Тесты идут через `scripts/t.sh` — харнесс из [скилла tests](https://github.com/rokokol/tests-skill): статус прогона честный, лог читается даже при exit 0, `just falsify` ломает гарантии из `tests/defects.sh` и требует, чтобы сьют это заметил

Веса моделей распознавания в репозиторий не входят и скачиваются отдельно, их лицензии перечислены в `THIRD-PARTY.md`
