#!/usr/bin/env bash
# Needs bash 3.2 and POSIX tools only, plus unsquashfs from squashfs-tools 4.4 or newer
#
# The check reads the permissions straight from the squashfs and mounts nothing. Under the
# FUSE runtime of AppImage, the files look like the user's own to the process that started
# the image, so a run on your own machine passes even with broken permissions. The files
# break for other users: firejail --appimage and the test of the appimage.github.io catalog
# mount the image as root, but run it as an ordinary user
set -euo pipefail

usage() {
  cat <<'EOF'
check-appimage.sh — can any user read and run an AppImage

  check-appimage.sh FILE...    check the permissions of the files inside each image

A directory must be readable and enterable by everyone. An ordinary file must be
readable by everyone, and an executable file must also be runnable by everyone. Each
violation is printed as a line from the unsquashfs listing

Needs unsquashfs from squashfs-tools 4.4 or newer
No network is needed
Exit 0 the permissions are fine, 1 a violation was found, 2 a call error or a file is not an AppImage
EOF
}

fail() { # the image violates the permissions
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 1
}

die() { # the request is wrong
  printf 'check-appimage.sh: %s\n' "$1" >&2
  exit 2
}

# An unsigned little-endian number of COUNT bytes at the offset OFFSET
read_le() {
  local file="$1" offset="$2" count="$3" value=0 i
  local -a bytes
  # `od -t u8` cannot be used here: the BSD od has no eight-byte integers. The bytes are read one by one
  # shellcheck disable=SC2207 # od prints only decimal numbers, so splitting by words is safe
  bytes=($(od -An -v -t u1 -j "$offset" -N "$count" -- "$file"))
  ((${#bytes[@]} == count)) || die "$file: the file is shorter than an ELF header"
  for ((i = count - 1; i >= 0; i--)); do
    value=$((value * 256 + bytes[i]))
  done
  printf '%s\n' "$value"
}

# The squashfs starts right after the ELF runtime, and the runtime ends with the table of
# section headers. The runtime counts the same way in --appimage-offset. The image must not
# be run for this: on NixOS, binfmt intercepts the start through appimage-run
squashfs_offset() {
  local file="$1" magic class shoff shentsize shnum
  magic=$(od -An -c -N 4 -- "$file" | tr -d ' ')
  [[ "$magic" == '177ELF' ]] || die "$file: not an ELF, so not an AppImage"
  # `|| exit`: the function itself runs inside $(...), where set -e is not inherited
  class=$(read_le "$file" 4 1) || exit
  ((class == 2)) || die "$file: a 32-bit ELF is not supported"
  shoff=$(read_le "$file" 40 8) || exit
  shentsize=$(read_le "$file" 58 2) || exit
  shnum=$(read_le "$file" 60 2) || exit
  printf '%s\n' $((shoff + shentsize * shnum))
}

violations=0

# Violations pile up in violations and not in the status of a function: a call in `if` or `||`
# would turn set -e off, and a die from a substitution would become a permission violation instead of a call error
check_one() {
  local file="$1" offset listing mode rest
  [[ -f "$file" ]] || die "$file: no such file"
  offset=$(squashfs_offset "$file")
  listing=$(unsquashfs -lln -o "$offset" "$file") ||
    die "$file: unsquashfs could not read the squashfs at offset $offset"
  [[ -n "$listing" ]] || die "$file: the listing is empty, nothing to check"
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
  -*) die "no such flag: $1" ;;
esac

for file in "$@"; do
  check_one "$file"
done
((violations == 0)) || fail "violations: $violations, not every user can read or run the image"
