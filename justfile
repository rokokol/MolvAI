# Единый источник правды для гейта: CI вызывает эти же рецепты.
# Тестовые прогоны идут через scripts/t.sh (скилл tests): статус — собственный статус cargo,
# лог читается даже при exit 0.

set shell := ["bash", "-euo", "pipefail", "-c"]

build_cmd := "cargo build --workspace --all-targets"
test_cmd := "cargo test --workspace --no-fail-fast"

default: check

# Полный гейт: форматирование, линтер, тесты, SPDX-заголовки, отсутствие заглушек
check: fmt clippy test spdx-check no-stubs

fmt:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets -- -D warnings

build:
    {{build_cmd}}

# Тесты с честным вердиктом и прочтённым логом
test:
    scripts/t.sh run -- {{test_cmd}}

# N прогонов подряд: расходятся ли результаты на одном и том же коде
flaky N:
    scripts/t.sh flaky {{N}} -- {{test_cmd}}

# Ломаем гарантии из tests/defects.sh по одной и требуем, чтобы сьют заметил
falsify *FILTER:
    scripts/t.sh falsify -d tests/defects.sh -b '{{build_cmd}}' {{FILTER}} -- {{test_cmd}}

# Какой коммит сломал сьют, начиная с GOOD
bisect GOOD:
    scripts/t.sh bisect {{GOOD}} -b '{{build_cmd}}' -- {{test_cmd}}

# У каждого исходника есть SPDX-заголовок
spdx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    missing=$(grep -rL --include='*.rs' --include='*.ts' --include='*.tsx' \
        --exclude-dir=target --exclude-dir=node_modules --exclude-dir=dist --exclude-dir=gen \
        'SPDX-License-Identifier: MIT' crates || true)
    if [ -n "$missing" ]; then
        echo "Нет SPDX-заголовка:"; echo "$missing"; exit 1
    fi
    echo "SPDX: ok"

# В коде продукта нет заглушек todo!/unimplemented!
no-stubs:
    #!/usr/bin/env bash
    set -euo pipefail
    if grep -rnE 'todo!\(|unimplemented!\(' --include='*.rs' --exclude-dir=target crates | grep -v '/tests/'; then
        echo "Заглушки в коде продукта"; exit 1
    fi
    echo "Заглушек нет"

deny:
    cargo deny check

audit:
    cargo audit

# Покрытие как карта, не как гейт: артефакт для CI
cov:
    cargo llvm-cov --workspace --lcov --output-path coverage.lcov
