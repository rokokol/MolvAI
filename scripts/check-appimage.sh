#!/usr/bin/env bash
# Needs bash 3.2 and POSIX tools only, plus unsquashfs from squashfs-tools 4.4 or newer
#
# Проверка читает права прямо из squashfs и ничего не монтирует. Под FUSE-рантаймом
# AppImage файлы видны запустившему как свои, поэтому запуск на своей машине проходит
# и с битыми правами. Ломаются чужие: firejail --appimage и тест каталога
# appimage.github.io монтируют образ от root, а запускают от обычного пользователя
set -euo pipefail

usage() {
  cat <<'EOF'
check-appimage.sh — может ли любой пользователь прочитать и запустить AppImage

  check-appimage.sh FILE...    проверить права файлов внутри каждого образа

Каталог должен быть доступен всем на чтение и вход. Обычный файл должен быть
доступен всем на чтение, а исполняемый ещё и на запуск. Каждое нарушение
печатается строкой из листинга unsquashfs

Нужен unsquashfs из squashfs-tools 4.4 или новее
Сеть не нужна
Exit 0 права в порядке, 1 нашлось нарушение, 2 ошибка вызова или файл не AppImage
EOF
}

fail() { # образ нарушает права
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 1
}

die() { # запрос неверен
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 2
}

# Беззнаковое little-endian число из COUNT байт по смещению OFFSET
read_le() {
  local file="$1" offset="$2" count="$3" value=0 i
  local -a bytes
  # Здесь нельзя `od -t u8`: у BSD od нет восьмибайтовых целых. Байты читаются по одному
  # shellcheck disable=SC2207 # od печатает только десятичные числа, разбиение по словам безопасно
  bytes=($(od -An -v -t u1 -j "$offset" -N "$count" -- "$file"))
  ((${#bytes[@]} == count)) || die "$file: файл короче ELF-заголовка"
  for ((i = count - 1; i >= 0; i--)); do
    value=$((value * 256 + bytes[i]))
  done
  printf '%s\n' "$value"
}

# squashfs начинается сразу за ELF-рантаймом, а рантайм кончается таблицей
# заголовков секций. Так же считает и сам рантайм в --appimage-offset. Запускать
# образ ради этого нельзя: на NixOS binfmt перехватывает запуск через appimage-run
squashfs_offset() {
  local file="$1" magic class shoff shentsize shnum
  magic=$(od -An -c -N 4 -- "$file" | tr -d ' ')
  [[ "$magic" == '177ELF' ]] || die "$file: не ELF, значит не AppImage"
  # `|| exit`: функция сама работает внутри $(...), куда set -e не наследуется
  class=$(read_le "$file" 4 1) || exit
  ((class == 2)) || die "$file: 32-битный ELF не поддерживается"
  shoff=$(read_le "$file" 40 8) || exit
  shentsize=$(read_le "$file" 58 2) || exit
  shnum=$(read_le "$file" 60 2) || exit
  printf '%s\n' $((shoff + shentsize * shnum))
}

violations=0

# Нарушения копятся в violations, а не в статусе функции: вызов в `if` или `||`
# выключил бы set -e, и die из подстановки стал бы нарушением прав вместо ошибки вызова
check_one() {
  local file="$1" offset listing mode rest
  [[ -f "$file" ]] || die "$file: нет такого файла"
  offset=$(squashfs_offset "$file")
  listing=$(unsquashfs -lln -o "$offset" "$file") ||
    die "$file: unsquashfs не прочитал squashfs по смещению $offset"
  [[ -n "$listing" ]] || die "$file: пустой листинг, проверять нечего"
  while read -r mode rest; do
    case "$mode" in
      d*)
        [[ "${mode:7:1}" == r && "${mode:9:1}" == x ]] && continue
        ;;
      -*)
        if [[ "${mode:7:1}" == r ]]; then
          case "${mode:1:9}" in
            *x*) [[ "${mode:9:1}" == x ]] && continue ;;
            *) continue ;;
          esac
        fi
        ;;
      *) continue ;;
    esac
    printf '%s: %s %s\n' "$file" "$mode" "$rest" >&2
    violations=$((violations + 1))
  done <<<"$listing"
}

(($# > 0)) || {
  usage >&2
  exit 2
}
case "$1" in
  -h | --help | help)
    usage
    exit 0
    ;;
  -*) die "нет такого флага: $1" ;;
esac

for file in "$@"; do
  check_one "$file"
done
((violations == 0)) || fail "нарушений: $violations, не каждый пользователь сможет прочитать или запустить образ"
