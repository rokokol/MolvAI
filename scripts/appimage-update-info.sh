#!/usr/bin/env bash
# Needs bash 3.2, GNU objcopy, readelf, unsquashfs, zsyncmake and the
# appimage_squashfs_offset helper in lib/appimage-offset.sh
#
# AppImageUpdate reads the update information from the .upd_info ELF section, and the .zsync
# file next to the image is the delta index it downloads. The Tauri bundler writes neither.
#
# objcopy rewrites the program headers and drops every byte outside the section table, which
# for an AppImage is the whole squashfs. So the section goes into the runtime alone, and the
# payload is appended back afterwards; check-appimage.sh then proves the payload survived.
set -euo pipefail

UPDATE_INFO='gh-releases-zsync|rokokol|MolvAI|latest|MolvAI*.AppImage.zsync'

usage() {
  cat <<'EOF'
appimage-update-info.sh — make an AppImage update itself through AppImageUpdate

  appimage-update-info.sh FILE...   embed the update information and write FILE.zsync

For every image the script adds or replaces the .upd_info ELF section in the runtime, keeps
the squashfs payload after it, and writes the matching .zsync index next to the image. The
permissions inside the repacked image are checked again, because objcopy once truncated the
payload in silence

Needs GNU objcopy, readelf, unsquashfs from squashfs-tools and zsyncmake
No network is needed
Exit 0 both parts are in place, 1 an image does not carry them, 2 on a usage error
EOF
}

fail() { # the image does not carry the update information
  printf 'appimage-update-info.sh: %s\n' "$1" >&2
  exit 1
}

die() { # the request itself is wrong
  printf 'appimage-update-info.sh: %s\n' "$1" >&2
  exit 2
}

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=lib/appimage-offset.sh
. "$HERE/lib/appimage-offset.sh"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/appimage-update.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# Whether the section table holds .upd_info. `objcopy --dump-section` must never be run on
# a whole AppImage: with no output file it rewrites the input and drops the squashfs
has_section() { # has_section FILE
  local table
  table=$(readelf -S "$1" 2>/dev/null) || return 1
  grep -q '\.upd_info' <<<"$table"
}

# The text of .upd_info, read without touching the file
section_text() { # section_text FILE
  local text
  text=$(readelf -p .upd_info "$1" 2>/dev/null | sed -n 's/^  \[[^]]*\]  //p') || return 1
  printf '%s\n' "${text%%$'\n'*}"
}

write_section() { # write_section RUNTIME
  printf '%s\0' "$UPDATE_INFO" >"$WORK/upd_info"
  if has_section "$1"; then
    objcopy --update-section .upd_info="$WORK/upd_info" "$1"
  else
    objcopy --add-section .upd_info="$WORK/upd_info" \
      --set-section-flags .upd_info=noload,readonly "$1"
  fi
}

repack() { # repack FILE OFFSET -> a new image with the section and the payload
  head -c "$2" "$1" >"$WORK/runtime"
  tail -c "+$(($2 + 1))" "$1" >"$WORK/payload"
  [[ -s "$WORK/payload" ]] || die "$1: the squashfs payload is empty"
  write_section "$WORK/runtime"
  cat "$WORK/runtime" "$WORK/payload" >"$WORK/image"
  chmod 755 "$WORK/image"
  local runtime_size
  runtime_size=$(wc -c <"$WORK/runtime" | tr -d ' ')
  local new_offset
  new_offset=$(appimage_squashfs_offset "$WORK/image") || die "$1: the repacked image is not an ELF"
  [[ "$new_offset" == "$runtime_size" ]] ||
    die "$1: the payload moved: offset $new_offset, runtime $runtime_size"
  mv "$WORK/image" "$1"
}

check_one() {
  local file="$1" offset got
  [[ -f "$file" ]] || die "$file: no such file"
  [[ "$file" == *.AppImage ]] || die "$file: the name does not end in .AppImage"
  if ! offset=$(appimage_squashfs_offset "$file"); then
    exit 2
  fi
  repack "$file" "$offset"

  has_section "$file" || fail "$file: the .upd_info section is missing after the write"
  got=$(section_text "$file")
  [[ "$got" == "$UPDATE_INFO" ]] ||
    fail "$file: .upd_info holds '$got' instead of '$UPDATE_INFO'"
  "$HERE/check-appimage.sh" "$file"
  (cd -- "$(dirname -- "$file")" && zsyncmake -u "$(basename -- "$file")" \
    -o "$(basename -- "$file").zsync" "$(basename -- "$file")")
  [[ -s "$file.zsync" ]] || fail "$file: the .zsync index is missing or empty"
}

(($# > 0)) || {
  usage >&2
  exit 2
}
case "${1:-}" in
  -h | --help | help)
    usage
    exit 0
    ;;
  -*) die "no such flag: $1" ;;
esac
command -v objcopy >/dev/null || die "objcopy is not on PATH"
command -v readelf >/dev/null || die "readelf is not on PATH"
command -v unsquashfs >/dev/null || die "unsquashfs is not on PATH"
command -v zsyncmake >/dev/null || die "zsyncmake is not on PATH"
for file in "$@"; do
  check_one "$file"
done
