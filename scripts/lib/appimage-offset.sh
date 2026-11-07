#!/usr/bin/env bash
# Source-only helpers for AppImage files: the offset of the squashfs payload.
#
# An AppImage type 2 is an ELF runtime followed by a squashfs. The runtime ends with the
# section header table, so the payload starts at e_shoff + e_shnum * e_shentsize. Reading
# this here keeps the permission check and the update-info writer on one definition.

# An unsigned little-endian number of COUNT bytes at OFFSET
read_le() {
  local file="$1" offset="$2" count="$3" value=0 i
  local -a bytes
  # `od -t u8` cannot be used here: the BSD od has no eight-byte integers. The bytes
  # are read one by one
  # shellcheck disable=SC2207 # od prints only decimal numbers, so splitting by words is safe
  bytes=($(od -An -v -t u1 -j "$offset" -N "$count" -- "$file"))
  ((${#bytes[@]} == count)) || return 2
  for ((i = count - 1; i >= 0; i--)); do
    value=$((value * 256 + bytes[i]))
  done
  printf '%s\n' "$value"
}

# The offset where the squashfs starts. Prints the reason to stderr and returns 2 when the
# file is not a 64-bit ELF
appimage_squashfs_offset() {
  local file="$1" magic class shoff shentsize shnum
  magic=$(od -An -c -N 4 -- "$file" | tr -d ' ')
  if [[ "$magic" != '177ELF' ]]; then
    printf '%s: not an ELF, so not an AppImage\n' "$file" >&2
    return 2
  fi
  class=$(read_le "$file" 4 1) || return 2
  if ((class != 2)); then
    printf '%s: a 32-bit ELF is not supported\n' "$file" >&2
    return 2
  fi
  shoff=$(read_le "$file" 40 8) || return 2
  shentsize=$(read_le "$file" 58 2) || return 2
  shnum=$(read_le "$file" 60 2) || return 2
  printf '%s\n' $((shoff + shentsize * shnum))
}
