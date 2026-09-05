#!/usr/bin/env bash
# Установка MolvAI на систему без Nix. Скрипт делает ровно то же, что даёт `nix develop`
# плюс `cargo build`: кладёт бинарник в префикс пользователя, при необходимости — файл
# рабочего стола и автозапуск, и записывает манифест, по которому --uninstall всё снимает.
#
# Зависимости скрипт не устанавливает никогда: он собирает список недостающего и печатает
# точные команды для дистрибутива, а решение принимает человек.
#
# Форма скрипта — из скилла huix-standard.
set -euo pipefail

here="$(cd -- "$(dirname -- "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"

# Версия живёт в одном месте — Cargo.toml рабочего пространства. Второй источник
# разъехался бы с первым, поэтому здесь она читается, а не повторяется
VERSION="$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\([^"]*\)".*/\1/p' \
  "$here/Cargo.toml" | head -n 1)"
if [[ -z "$VERSION" ]]; then
  echo "install.sh: не удалось прочитать версию из Cargo.toml — сломался разбор, а не проект" >&2
  exit 2
fi

PREFIX="${PREFIX:-$HOME/.local}"
DESTDIR="${DESTDIR:-}"
OS_RELEASE="${OS_RELEASE:-/etc/os-release}"

usage() {
  cat <<EOF
установка MolvAI $VERSION в префикс пользователя

Каждый запуск приводит префикс ровно к тем флагам, которые заданы: запуск без флага
снимает то, что этот флаг ставил в прошлый раз.

использование: ./install.sh [опции]
  -h, --help          показать эту справку и выйти
  -v, --version       напечатать версию и выйти
  -c, --check         только проверить зависимости, ничего не устанавливать
      --prefix DIR    префикс установки (сейчас: $PREFIX; переменная PREFIX)
      --destdir DIR   промежуточный корень: файлы ложатся в DESTDIR/PREFIX,
                      живая система не трогается (переменная DESTDIR)
      --desktop       поставить файл рабочего стола в меню приложений
      --autostart     запускать MolvAI при входе в систему
      --uninstall     снять всё, что записал прошлый запуск, по манифесту
      --purge         вместе с --uninstall: снести настройки и журнал реплик
      --keep-history  вместе с --uninstall: сохранить настройки и журнал без вопроса

Права администратора не нужны: по умолчанию всё ложится в \$HOME/.local.
Установка поверх сохраняет настройки и журнал — они лежат вне префикса.
EOF
}

UNINSTALL=0
CHECK_ONLY=0
DESKTOP=0
AUTOSTART=0
PURGE=0
KEEP_HISTORY=0
config_given=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h | --help)
      usage
      exit 0
      ;;
    -v | --version)
      echo "MolvAI $VERSION"
      exit 0
      ;;
    -c | --check)
      CHECK_ONLY=1
      shift
      ;;
    --prefix)
      PREFIX="${2:?нужен каталог после $1}"
      shift 2
      ;;
    --destdir)
      DESTDIR="${2:?нужен каталог после $1}"
      shift 2
      ;;
    --desktop)
      DESKTOP=1
      config_given="$1"
      shift
      ;;
    --autostart)
      AUTOSTART=1
      config_given="$1"
      shift
      ;;
    --uninstall)
      UNINSTALL=1
      shift
      ;;
    --purge)
      PURGE=1
      shift
      ;;
    --keep-history)
      KEEP_HISTORY=1
      shift
      ;;
    *)
      echo "install.sh: неизвестная опция: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$PREFIX" != /* ]]; then
  echo "install.sh: PREFIX должен быть абсолютным путём: $PREFIX" >&2
  exit 1
fi
if ((UNINSTALL)) && [[ -n "$config_given" ]]; then
  echo "install.sh: --uninstall не сочетается с $config_given" >&2
  exit 1
fi
if ((PURGE)) && ((KEEP_HISTORY)); then
  echo "install.sh: --purge и --keep-history взаимно исключают друг друга" >&2
  exit 1
fi

root="${DESTDIR%/}$PREFIX"
share_runtime="$PREFIX/share/molva"
share="${DESTDIR%/}$share_runtime"
manifest="$share/install-manifest"

config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/molva"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}/molva"
autostart_file="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/molva.desktop"

# --- манифест ---------------------------------------------------------------------------
# Каждый созданный путь записывается в конечном виде, без DESTDIR: манифест едет внутри
# промежуточного дерева и остаётся верным там, где это дерево окажется

old_paths=()
if [[ -f "$manifest" ]]; then
  mapfile -t old_paths < <(grep -v '^#' "$manifest")
fi

installed=()

put() { # put РЕЖИМ ИСТОЧНИК КОНЕЧНЫЙ_ПУТЬ
  install -D -m "$1" "$2" "${DESTDIR%/}$3"
  installed+=("$3")
}

# write_file РЕЖИМ КОНЕЧНЫЙ_ПУТЬ СОДЕРЖИМОЕ. Содержимое передаётся аргументом, а не по
# конвейеру: правая часть конвейера — подоболочка, и запись в манифест из неё пропадала
write_file() {
  install -d "$(dirname "${DESTDIR%/}$2")"
  printf '%s\n' "$3" >"${DESTDIR%/}$2"
  chmod "$1" "${DESTDIR%/}$2"
  installed+=("$2")
}

prune() { # убрать опустевшие родительские каталоги, не выходя за префикс
  local dir stop
  dir="$(dirname "${DESTDIR%/}$1")"
  stop="$root"
  while [[ "$dir" == "$stop"/* ]]; do
    rmdir "$dir" 2>/dev/null || break
    dir="$(dirname "$dir")"
  done
}

# --- удаление ---------------------------------------------------------------------------

if ((UNINSTALL)); then
  if [[ ! -f "$manifest" ]]; then
    echo "MolvAI: нечего удалять в $root — манифеста установки нет"
    exit 0
  fi

  # Демон держит горячую клавишу; пока он жив, снимать файлы бессмысленно
  if [[ -z "$DESTDIR" ]] && command -v molva >/dev/null 2>&1; then
    molva quit >/dev/null 2>&1 || true
  fi

  removed=()
  while IFS= read -r path; do
    [[ -z "$path" || "$path" == \#* ]] && continue
    if [[ -e "${DESTDIR%/}$path" || -L "${DESTDIR%/}$path" ]]; then
      removed+=("$path")
    fi
    rm -f "${DESTDIR%/}$path"
    prune "$path"
  done <"$manifest"
  rm -f "$manifest"
  rmdir "$share" 2>/dev/null || true

  echo "MolvAI $VERSION снят с $root"
  echo "Снятые компоненты:"
  if ((${#removed[@]})); then
    printf '  - %s\n' "${removed[@]}"
  else
    echo "  - ничего: файлы уже отсутствовали"
  fi

  # У промежуточного дерева нет живого состояния: настройки и журнал его не касаются
  if [[ -n "$DESTDIR" ]]; then
    exit 0
  fi

  # Настройки и журнал живут вне префикса: их судьбу решает пользователь, а не манифест
  history_paths=()
  [[ -e "$config_dir" ]] && history_paths+=("$config_dir")
  [[ -e "$data_dir" ]] && history_paths+=("$data_dir")

  if ((${#history_paths[@]} == 0)); then
    echo "Настроек и журнала не найдено — удалять нечего"
    exit 0
  fi

  if ((KEEP_HISTORY)); then
    echo "Сохранены (--keep-history):"
    printf '  - %s\n' "${history_paths[@]}"
    exit 0
  fi

  if ! ((PURGE)); then
    echo
    echo "Остались настройки и журнал реплик:"
    printf '  - %s\n' "${history_paths[@]}"
    if [[ -t 0 ]]; then
      read -r -p "Удалить их тоже? [y/N] " answer
      case "$answer" in
        [yY] | [yY][eE][sS]) PURGE=1 ;;
      esac
    else
      echo "Ввод недоступен, история сохранена: повторите с --purge или --keep-history"
      exit 0
    fi
  fi

  if ((PURGE)); then
    rm -rf "${history_paths[@]}"
    echo "Удалены:"
    printf '  - %s\n' "${history_paths[@]}"
  else
    echo "Сохранены:"
    printf '  - %s\n' "${history_paths[@]}"
  fi
  exit 0
fi

# --- проверка зависимостей: громко отказаться, ничего не поставив ------------------------
# Обязательные — без них установка невозможна; сессионные — их даёт рабочая сессия
# пользователя, по одному предупреждению и установка продолжается

missing=()
absent=()

need() { command -v "$1" >/dev/null 2>&1 || missing+=("$1"); }
want() { command -v "$1" >/dev/null 2>&1 || absent+=("$1"); }

need install
need sed

binary="$here/target/release/molva"
if [[ ! -x "$binary" ]]; then
  need cargo
  need cc
fi

# Способ вставки текста зависит от типа сессии, поэтому и проверяется по нему
case "${XDG_SESSION_TYPE:-}" in
  wayland)
    want wtype
    want wl-copy
    ;;
  x11)
    want xdotool
    want xclip
    ;;
esac

distro_id() {
  sed -n 's/^ID\(_LIKE\)\?=//p' "$OS_RELEASE" 2>/dev/null | tr -d '"' | tr '\n' ' '
}

guidance() {
  # Один рекомендованный способ на систему. Запускаемые строки печатаются как `  $ команда`
  # — два пробела, доллар, пробел: этот формат читают и человек, и тест
  case "$(uname -s)" in
    Darwin)
      echo "Поставьте их на macOS:"
      echo '  $ brew install cmake pkg-config'
      echo "  плюс инструменты командной строки Xcode: xcode-select --install"
      return
      ;;
    MINGW* | MSYS* | CYGWIN*)
      echo "Поставьте их на Windows:"
      echo '  $ winget install Rustlang.Rustup Kitware.CMake'
      echo "  плюс Build Tools for Visual Studio с рабочей нагрузкой C++"
      return
      ;;
  esac
  case " $(distro_id) " in
    *" nixos "*)
      echo "На NixOS всё нужное даёт флейк репозитория, ставить в систему ничего не надо:"
      echo '  $ nix develop --command ./install.sh'
      ;;
    *" arch "*)
      echo "Поставьте их на Arch:"
      echo '  $ sudo pacman -S --needed base-devel cmake pkgconf alsa-lib libxkbcommon rustup'
      echo "Для Wayland дополнительно:"
      echo '  $ sudo pacman -S --needed wtype wl-clipboard'
      ;;
    *" debian "* | *" ubuntu "*)
      echo "Поставьте их на Debian или Ubuntu:"
      echo '  $ sudo apt install build-essential cmake pkg-config libasound2-dev libxkbcommon-dev rustup'
      echo "Для Wayland дополнительно:"
      echo '  $ sudo apt install wtype wl-clipboard'
      ;;
    *" fedora "* | *" rhel "*)
      echo "Поставьте их на Fedora:"
      echo '  $ sudo dnf install gcc-c++ cmake pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel rustup'
      echo "Для Wayland дополнительно:"
      echo '  $ sudo dnf install wtype wl-clipboard'
      ;;
    *" suse "* | *" opensuse "*)
      echo "Поставьте их на openSUSE:"
      echo '  $ sudo zypper install gcc-c++ cmake pkg-config alsa-devel libxkbcommon-devel rustup'
      echo "Для Wayland дополнительно:"
      echo '  $ sudo zypper install wtype wl-clipboard'
      ;;
    *)
      echo "Поставьте их пакетным менеджером вашей системы:"
      echo "  компилятор C и C++, cmake, pkg-config, заголовки ALSA, rustup"
      echo "  https://rustup.rs"
      ;;
  esac
}

if ((${#missing[@]})); then
  {
    echo "install.sh: не хватает зависимостей:"
    printf '  - %s\n' "${missing[@]}"
    echo
    guidance
  } >&2
  exit 1
fi

if ((${#absent[@]})); then
  printf 'install.sh: не найдено (даёт рабочая сессия, установка продолжается): %s\n' \
    "${absent[@]}" >&2
  echo "install.sh: без них вставка текста упадёт на буфер обмена — рабочий режим, но не тот" >&2
fi

if ((CHECK_ONLY)); then
  echo "install.sh: обязательные зависимости на месте, установка возможна"
  exit 0
fi

# --- установка --------------------------------------------------------------------------

if [[ ! -x "$binary" ]]; then
  echo "install.sh: собираю MolvAI $VERSION (это надолго в первый раз)"
  (cd "$here" && cargo build --locked --release --bin molva)
fi
if [[ ! -x "$binary" ]]; then
  echo "install.sh: сборка не дала $binary" >&2
  exit 1
fi

put 755 "$binary" "$PREFIX/bin/molva"
put 644 "$here/LICENSE" "$share_runtime/LICENSE"
put 644 "$here/NOTICE" "$share_runtime/NOTICE"
put 644 "$here/THIRD-PARTY.md" "$share_runtime/THIRD-PARTY.md"

desktop_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=MolvAI
Comment=Системный голосовой ввод
Exec=$PREFIX/bin/molva daemon
Terminal=false
Categories=Utility;Accessibility;
X-GNOME-Autostart-enabled=true
EOF
}

if ((DESKTOP)) || ((AUTOSTART)); then
  write_file 644 "$PREFIX/share/applications/molva.desktop" "$(desktop_entry)"
fi

if ((AUTOSTART)); then
  if [[ -n "$DESTDIR" ]]; then
    echo "install.sh: --autostart пропущен: с --destdir живое состояние не трогается" >&2
  else
    write_file 644 "$autostart_file" "$(desktop_entry)"
  fi
fi

# Объявительный смысл флагов: что прошлый запуск поставил, а этот — нет, снимается
for path in "${old_paths[@]}"; do
  keep=0
  for now in "${installed[@]}"; do
    [[ "$path" == "$now" ]] && keep=1
  done
  ((keep)) || {
    rm -f "${DESTDIR%/}$path"
    prune "$path"
  }
done

{
  echo "# манифест установки MolvAI $VERSION"
  printf '%s\n' "${installed[@]}"
} >"$manifest"

echo "MolvAI $VERSION установлен в $root"
printf '  - %s\n' "${installed[@]}"
echo "Манифест: $manifest"

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *)
    echo
    echo "install.sh: $PREFIX/bin не в PATH. Добавьте в файл вашей оболочки:"
    echo "  export PATH=\"$PREFIX/bin:\$PATH\""
    ;;
esac

echo
echo "Дальше: molva model download small && molva devices && molva test-inject"
echo "Удалить: $here/install.sh --uninstall"
